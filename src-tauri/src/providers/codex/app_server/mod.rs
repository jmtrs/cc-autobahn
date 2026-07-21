//! Official Codex account sensor over an owned App Server stdio child.
//!
//! Wire assumptions live in the `wire` (JSON-RPC framing) and `normalize`
//! (payload shaping) submodules. The rest of cc-autobahn consumes normalized,
//! provider-discriminated snapshots and can keep using rollout/ccusage when
//! this version- and authentication-dependent source is unavailable.

mod normalize;
mod wire;

use std::collections::HashMap;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager};

use self::normalize::{
    hook_health, merge_rate_limit_update, normalize_hook_probe, normalize_limits, normalize_usage,
};
use self::wire::{
    compact_error, read_messages, response_id, send_notification, send_request, ReaderEvent,
};
use super::ID;
use crate::providers::{
    emit_health, now_epoch_ms, AccountUsageSnapshot, HealthStatus, ProviderComponent,
    RateLimitSnapshot, SourceQuality,
};

const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_secs(60);
const HOOKS_POLL_INTERVAL: Duration = Duration::from_secs(5);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const STALE_AFTER: Duration = Duration::from_secs(150);
const UNAVAILABLE_AFTER_MS: i64 = 10 * 60 * 1000;
const MAX_BACKOFF_SECS: u64 = 60;

static ACTIVE_CHILD: Mutex<Option<Child>> = Mutex::new(None);
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountSensorSnapshot {
    pub rate_limits: Option<RateLimitSnapshot>,
    pub account_usage: Option<AccountUsageSnapshot>,
    pub permission_hook: Option<CodexHookProbe>,
    pub permission_hook_observed_at_ms: Option<i64>,
    pub runtime: CodexRuntimeDiagnostics,
    #[serde(skip)]
    pub(crate) hook_inventory_generation: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexRuntimeDiagnostics {
    pub executable_path: Option<String>,
    pub version: Option<String>,
    pub connection_status: String,
    pub rate_limits_available: Option<bool>,
    pub rate_limits_reason: Option<String>,
    pub account_usage_available: Option<bool>,
    pub account_usage_reason: Option<String>,
    pub hooks_inventory_available: Option<bool>,
    pub hooks_inventory_reason: Option<String>,
    pub observed_at_ms: i64,
}

impl Default for CodexRuntimeDiagnostics {
    fn default() -> Self {
        Self {
            executable_path: None,
            version: None,
            connection_status: "unavailable".into(),
            rate_limits_available: None,
            rate_limits_reason: None,
            account_usage_available: None,
            account_usage_reason: None,
            hooks_inventory_available: None,
            hooks_inventory_reason: None,
            observed_at_ms: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexHookProbe {
    pub enabled: bool,
    pub trust_status: String,
    pub source_path: String,
    pub current_hash: Option<String>,
    pub observed_at_ms: i64,
}

pub type AccountSensorState = Arc<Mutex<AccountSensorSnapshot>>;

pub fn new_state() -> AccountSensorState {
    Arc::new(Mutex::new(AccountSensorSnapshot::default()))
}

#[tauri::command]
pub fn codex_account_snapshot(app: AppHandle) -> AccountSensorSnapshot {
    app.state::<AccountSensorState>()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

pub fn start(app: AppHandle) {
    thread::spawn(move || {
        let state = app.state::<AccountSensorState>().inner().clone();
        let mut backoff_secs = 1u64;
        loop {
            if SHUTTING_DOWN.load(Ordering::Acquire) {
                return;
            }
            let path = app
                .try_state::<crate::path_state::PathState>()
                .and_then(|state| crate::path_state::get(&state));
            let Some(executable) = resolve_executable("codex", path.as_deref()) else {
                update_runtime(&state, |runtime| {
                    runtime.executable_path = None;
                    runtime.version = None;
                    runtime.connection_status = "unavailable".into();
                    runtime.rate_limits_available = None;
                    runtime.rate_limits_reason = Some("Codex executable not found".into());
                    runtime.account_usage_available = None;
                    runtime.account_usage_reason = Some("Codex executable not found".into());
                    runtime.hooks_inventory_available = None;
                    runtime.hooks_inventory_reason = Some("Codex executable not found".into());
                });
                clear_hook_probe(&state);
                emit_health(
                    &app,
                    ID,
                    ProviderComponent::AppServer,
                    HealthStatus::Unavailable,
                    Some("Codex executable not found".into()),
                );
                emit_health(
                    &app,
                    ID,
                    ProviderComponent::Permissions,
                    HealthStatus::Unavailable,
                    Some("Codex executable not found; hook inventory unavailable".into()),
                );
                mark_stale_if_expired(&app, &state);
                mark_unavailable_if_expired(&app, &state);
                thread::sleep(Duration::from_secs(MAX_BACKOFF_SECS));
                continue;
            };
            let version = runtime_version(&executable, path.as_deref())
                .unwrap_or_else(|| "unknown Codex version".into());
            update_runtime(&state, |runtime| {
                runtime.executable_path = Some(executable.to_string_lossy().into_owned());
                runtime.version = Some(version.clone());
                runtime.connection_status = "connecting".into();
                runtime.rate_limits_available = None;
                runtime.rate_limits_reason = None;
                runtime.account_usage_available = None;
                runtime.account_usage_reason = None;
                runtime.hooks_inventory_available = None;
                runtime.hooks_inventory_reason = None;
            });
            let connection_started = Instant::now();
            let result = run_connection(&app, &state, &executable, path.as_deref(), &version);
            if SHUTTING_DOWN.load(Ordering::Acquire) {
                return;
            }
            if connection_started.elapsed() >= POLL_INTERVAL {
                backoff_secs = 1;
            }
            mark_stale_if_expired(&app, &state);
            mark_unavailable_if_expired(&app, &state);
            clear_hook_probe(&state);
            let disconnect_reason = match &result {
                Ok(()) => format!("{version} disconnected"),
                Err(error) => format!("{version}: {error}"),
            };
            update_runtime(&state, |runtime| {
                runtime.connection_status = "disconnected".into();
                runtime.rate_limits_available = Some(false);
                runtime.rate_limits_reason = Some(disconnect_reason.clone());
                runtime.account_usage_available = Some(false);
                runtime.account_usage_reason = Some(disconnect_reason.clone());
                runtime.hooks_inventory_available = Some(false);
                runtime.hooks_inventory_reason = Some(disconnect_reason.clone());
            });
            emit_health(
                &app,
                ID,
                ProviderComponent::Permissions,
                HealthStatus::Degraded,
                Some("Codex hook inventory disconnected".into()),
            );
            emit_health(
                &app,
                ID,
                ProviderComponent::AppServer,
                HealthStatus::Degraded,
                Some(disconnect_reason),
            );
            thread::sleep(Duration::from_secs(backoff_secs));
            backoff_secs = (backoff_secs * 2).min(MAX_BACKOFF_SECS);
        }
    });
}

pub fn stop() {
    SHUTTING_DOWN.store(true, Ordering::Release);
    if let Some(mut child) = ACTIVE_CHILD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
    {
        let _ = child.kill();
        let _ = child.wait();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RequestKind {
    Initialize,
    RateLimits,
    AccountUsage,
    Hooks,
}

#[derive(Debug, Clone, Copy)]
struct PendingRequest {
    kind: RequestKind,
    sent_at: Instant,
    hook_inventory_generation: Option<u64>,
}

struct ActiveChildGuard;

impl Drop for ActiveChildGuard {
    fn drop(&mut self) {
        if let Some(mut child) = ACTIVE_CHILD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn run_connection(
    app: &AppHandle,
    state: &AccountSensorState,
    executable: &Path,
    path: Option<&str>,
    version: &str,
) -> Result<(), String> {
    let mut command = Command::new(executable);
    command
        .args(["app-server", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    {
        let mut active = ACTIVE_CHILD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if SHUTTING_DOWN.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Err("application shutting down".into());
        }
        *active = Some(child);
    }
    let _child_guard = ActiveChildGuard;
    let (stdout, mut stdin) = {
        let mut active = ACTIVE_CHILD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let child = active.as_mut().ok_or("App Server stopped during startup")?;
        (
            child.stdout.take().ok_or("App Server stdout unavailable")?,
            child.stdin.take().ok_or("App Server stdin unavailable")?,
        )
    };
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || read_messages(BufReader::new(stdout), sender));

    let mut next_id = 1u64;
    let mut pending = HashMap::new();
    send_request(
        &mut stdin,
        next_id,
        "initialize",
        json!({
            "clientInfo": {
                "name": "cc-autobahn",
                "title": "cc-autobahn",
                "version": env!("CARGO_PKG_VERSION")
            },
            "capabilities": { "experimentalApi": false }
        }),
    )?;
    pending.insert(
        next_id,
        PendingRequest {
            kind: RequestKind::Initialize,
            sent_at: Instant::now(),
            hook_inventory_generation: None,
        },
    );
    next_id += 1;

    let initialized_at = Instant::now();
    loop {
        if initialized_at.elapsed() >= INITIALIZE_TIMEOUT {
            return Err("initialize timed out".into());
        }
        let event =
            receiver
                .recv_timeout(Duration::from_millis(250))
                .map_err(|error| match error {
                    mpsc::RecvTimeoutError::Timeout => String::new(),
                    mpsc::RecvTimeoutError::Disconnected => "stdout reader stopped".into(),
                });
        match event {
            Ok(ReaderEvent::Message(message)) => {
                let Some(id) = response_id(&message) else {
                    continue;
                };
                if pending.remove(&id).map(|pending| pending.kind) != Some(RequestKind::Initialize)
                {
                    continue;
                }
                if let Some(error) = message.get("error") {
                    return Err(format!("initialize rejected: {}", compact_error(error)));
                }
                send_notification(&mut stdin, "initialized")?;
                update_runtime(state, |runtime| {
                    runtime.connection_status = "connected".into();
                });
                break;
            }
            Ok(ReaderEvent::Closed(error)) => return Err(error),
            Err(error) if error.is_empty() => continue,
            Err(error) => return Err(error),
        }
    }

    emit_health(
        app,
        ID,
        ProviderComponent::AppServer,
        HealthStatus::Degraded,
        Some(format!("{version}: probing account capabilities")),
    );

    let mut raw_limits: Option<Value> = None;
    let mut last_limits_official: Option<Instant> = None;
    let mut last_usage_official: Option<Instant> = None;
    let mut last_poll = Instant::now() - POLL_INTERVAL;
    let mut last_hooks_poll = Instant::now() - HOOKS_POLL_INTERVAL;
    let mut limits_stale_emitted = false;
    let mut usage_stale_emitted = false;
    let mut rate_capability: Option<bool> = None;
    let mut usage_capability: Option<bool> = None;
    let hooks_cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let outcome = loop {
        if last_poll.elapsed() >= POLL_INTERVAL {
            queue_probe(
                &mut stdin,
                &mut pending,
                &mut next_id,
                RequestKind::RateLimits,
                "account/rateLimits/read",
            )?;
            queue_probe(
                &mut stdin,
                &mut pending,
                &mut next_id,
                RequestKind::AccountUsage,
                "account/usage/read",
            )?;
            last_poll = Instant::now();
        }
        if last_hooks_poll.elapsed() >= HOOKS_POLL_INTERVAL {
            let hook_inventory_generation = state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .hook_inventory_generation;
            queue_probe_with_params(
                &mut stdin,
                &mut pending,
                &mut next_id,
                RequestKind::Hooks,
                "hooks/list",
                json!({ "cwds": [hooks_cwd] }),
                Some(hook_inventory_generation),
            )?;
            last_hooks_poll = Instant::now();
        }

        match receiver.recv_timeout(Duration::from_secs(1)) {
            Ok(ReaderEvent::Message(message)) => {
                if let Some(id) = response_id(&message) {
                    let Some(pending_request) = pending.remove(&id) else {
                        continue;
                    };
                    let kind = pending_request.kind;
                    if let Some(error) = message.get("error") {
                        let reason = compact_error(error);
                        match kind {
                            RequestKind::RateLimits => {
                                rate_capability = Some(false);
                                update_capability_diagnostic(
                                    state,
                                    kind,
                                    false,
                                    Some(reason.clone()),
                                );
                            }
                            RequestKind::AccountUsage => {
                                usage_capability = Some(false);
                                update_capability_diagnostic(
                                    state,
                                    kind,
                                    false,
                                    Some(reason.clone()),
                                );
                            }
                            RequestKind::Hooks => {
                                if !fail_hook_probe_if_current(
                                    state,
                                    pending_request.hook_inventory_generation,
                                    Some(reason.clone()),
                                ) {
                                    continue;
                                }
                                emit_health(
                                    app,
                                    ID,
                                    ProviderComponent::Permissions,
                                    HealthStatus::Degraded,
                                    Some(format!("hooks/list unavailable: {}", reason)),
                                );
                                continue;
                            }
                            RequestKind::Initialize => continue,
                        }
                        emit_capability_health(
                            app,
                            version,
                            rate_capability,
                            usage_capability,
                            Some(&reason),
                        );
                        update_account_capabilities(state, rate_capability, usage_capability);
                        continue;
                    }
                    let Some(result) = message.get("result") else {
                        continue;
                    };
                    match kind {
                        RequestKind::RateLimits => {
                            raw_limits = Some(result.clone());
                            if let Some(snapshot) =
                                normalize_limits(result, SourceQuality::Official)
                            {
                                store_and_emit_limits(app, state, snapshot);
                                rate_capability = Some(true);
                                update_capability_diagnostic(state, kind, true, None);
                                update_account_capabilities(
                                    state,
                                    rate_capability,
                                    usage_capability,
                                );
                                emit_capability_health(
                                    app,
                                    version,
                                    rate_capability,
                                    usage_capability,
                                    None,
                                );
                                last_limits_official = Some(Instant::now());
                                limits_stale_emitted = false;
                            } else {
                                rate_capability = Some(false);
                                update_capability_diagnostic(
                                    state,
                                    kind,
                                    false,
                                    Some("incompatible rate-limit response".into()),
                                );
                                update_account_capabilities(
                                    state,
                                    rate_capability,
                                    usage_capability,
                                );
                                emit_capability_health(
                                    app,
                                    version,
                                    rate_capability,
                                    usage_capability,
                                    Some("incompatible rate-limit response"),
                                );
                            }
                        }
                        RequestKind::AccountUsage => {
                            if let Some(snapshot) = normalize_usage(result, SourceQuality::Official)
                            {
                                store_and_emit_usage(app, state, snapshot);
                                usage_capability = Some(true);
                                update_capability_diagnostic(state, kind, true, None);
                                update_account_capabilities(
                                    state,
                                    rate_capability,
                                    usage_capability,
                                );
                                emit_capability_health(
                                    app,
                                    version,
                                    rate_capability,
                                    usage_capability,
                                    None,
                                );
                                last_usage_official = Some(Instant::now());
                                usage_stale_emitted = false;
                            } else {
                                usage_capability = Some(false);
                                update_capability_diagnostic(
                                    state,
                                    kind,
                                    false,
                                    Some("incompatible account-usage response".into()),
                                );
                                update_account_capabilities(
                                    state,
                                    rate_capability,
                                    usage_capability,
                                );
                                emit_capability_health(
                                    app,
                                    version,
                                    rate_capability,
                                    usage_capability,
                                    Some("incompatible account-usage response"),
                                );
                            }
                        }
                        RequestKind::Hooks => {
                            store_hook_probe(
                                app,
                                state,
                                result,
                                pending_request.hook_inventory_generation,
                            );
                        }
                        RequestKind::Initialize => {}
                    }
                } else if message.get("method").and_then(Value::as_str)
                    == Some("account/rateLimits/updated")
                {
                    if let Some(incoming) = message.pointer("/params/rateLimits") {
                        merge_rate_limit_update(&mut raw_limits, incoming);
                        if let Some(snapshot) = raw_limits
                            .as_ref()
                            .and_then(|value| normalize_limits(value, SourceQuality::Official))
                        {
                            store_and_emit_limits(app, state, snapshot);
                            rate_capability = Some(true);
                            update_capability_diagnostic(
                                state,
                                RequestKind::RateLimits,
                                true,
                                None,
                            );
                            update_account_capabilities(state, rate_capability, usage_capability);
                            emit_capability_health(
                                app,
                                version,
                                rate_capability,
                                usage_capability,
                                None,
                            );
                            last_limits_official = Some(Instant::now());
                            limits_stale_emitted = false;
                        }
                    }
                }
            }
            Ok(ReaderEvent::Closed(error)) => break Err(error),
            Err(mpsc::RecvTimeoutError::Disconnected) => break Err("stdout reader stopped".into()),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }

        if !limits_stale_emitted
            && last_limits_official.is_some_and(|instant| instant.elapsed() >= STALE_AFTER)
        {
            mark_limits_quality(app, state, SourceQuality::Stale);
            emit_health(
                app,
                ID,
                ProviderComponent::AppServer,
                HealthStatus::Degraded,
                Some(format!("{version}: official rate limits stale")),
            );
            limits_stale_emitted = true;
        }
        if !usage_stale_emitted
            && last_usage_official.is_some_and(|instant| instant.elapsed() >= STALE_AFTER)
        {
            mark_usage_quality(app, state, SourceQuality::Stale);
            emit_health(
                app,
                ID,
                ProviderComponent::AppServer,
                HealthStatus::Degraded,
                Some(format!("{version}: official account usage stale")),
            );
            usage_stale_emitted = true;
        }
        mark_unavailable_if_expired(app, state);
        let timed_out = take_timed_out(&mut pending, REQUEST_TIMEOUT);
        let mut account_timeout = false;
        for request in timed_out {
            let kind = request.kind;
            match kind {
                RequestKind::Hooks => {
                    if !fail_hook_probe_if_current(
                        state,
                        request.hook_inventory_generation,
                        Some("hooks/list timed out".into()),
                    ) {
                        continue;
                    }
                    emit_health(
                        app,
                        ID,
                        ProviderComponent::Permissions,
                        HealthStatus::Degraded,
                        Some("hooks/list timed out".into()),
                    );
                }
                RequestKind::RateLimits => {
                    rate_capability = Some(false);
                    update_capability_diagnostic(
                        state,
                        kind,
                        false,
                        Some("account/rateLimits/read timed out".into()),
                    );
                    account_timeout = true;
                }
                RequestKind::AccountUsage => {
                    usage_capability = Some(false);
                    update_capability_diagnostic(
                        state,
                        kind,
                        false,
                        Some("account/usage/read timed out".into()),
                    );
                    account_timeout = true;
                }
                RequestKind::Initialize => {}
            }
        }
        if account_timeout {
            update_account_capabilities(state, rate_capability, usage_capability);
            emit_capability_health(
                app,
                version,
                rate_capability,
                usage_capability,
                Some("one or more capability probes timed out"),
            );
        }
        if SHUTTING_DOWN.load(Ordering::Acquire) {
            break Err("application shutting down".into());
        }
        let exited = ACTIVE_CHILD
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .as_mut()
            .and_then(|child| child.try_wait().ok())
            .flatten();
        if let Some(status) = exited {
            break Err(format!("App Server exited with {status}"));
        }
    };
    outcome
}

fn take_timed_out(
    pending: &mut HashMap<u64, PendingRequest>,
    timeout: Duration,
) -> Vec<PendingRequest> {
    let expired: Vec<_> = pending
        .iter()
        .filter(|(_, request)| request.sent_at.elapsed() >= timeout)
        .map(|(id, request)| (*id, *request))
        .collect();
    for (id, _) in &expired {
        pending.remove(id);
    }
    expired.into_iter().map(|(_, request)| request).collect()
}

fn queue_probe(
    stdin: &mut ChildStdin,
    pending: &mut HashMap<u64, PendingRequest>,
    next_id: &mut u64,
    kind: RequestKind,
    method: &str,
) -> Result<(), String> {
    queue_probe_with_params(stdin, pending, next_id, kind, method, Value::Null, None)
}

fn queue_probe_with_params(
    stdin: &mut ChildStdin,
    pending: &mut HashMap<u64, PendingRequest>,
    next_id: &mut u64,
    kind: RequestKind,
    method: &str,
    params: Value,
    hook_inventory_generation: Option<u64>,
) -> Result<(), String> {
    if pending
        .values()
        .any(|pending_request| pending_request.kind == kind)
    {
        return Ok(());
    }
    send_request(stdin, *next_id, method, params)?;
    pending.insert(
        *next_id,
        PendingRequest {
            kind,
            sent_at: Instant::now(),
            hook_inventory_generation,
        },
    );
    *next_id = next_id.wrapping_add(1).max(1);
    Ok(())
}

fn emit_capability_health(
    app: &AppHandle,
    version: &str,
    rate_limits: Option<bool>,
    account_usage: Option<bool>,
    error: Option<&str>,
) {
    let status = if rate_limits == Some(true) && account_usage == Some(true) {
        HealthStatus::Connected
    } else {
        HealthStatus::Degraded
    };
    let capabilities = [
        match rate_limits {
            Some(true) => "rate limits ready",
            Some(false) => "rate limits unavailable",
            None => "rate limits probing",
        },
        match account_usage {
            Some(true) => "account usage ready",
            Some(false) => "account usage unavailable",
            None => "account usage probing",
        },
    ]
    .join(", ");
    let detail = error.map_or_else(
        || format!("{version}: {capabilities}"),
        |error| format!("{version}: {capabilities} ({error})"),
    );
    emit_health(app, ID, ProviderComponent::AppServer, status, Some(detail));
}

fn update_runtime(state: &AccountSensorState, update: impl FnOnce(&mut CodexRuntimeDiagnostics)) {
    let mut snapshot = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    update(&mut snapshot.runtime);
    snapshot.runtime.observed_at_ms = now_epoch_ms();
}

fn update_account_capabilities(
    state: &AccountSensorState,
    rate_limits: Option<bool>,
    account_usage: Option<bool>,
) {
    update_runtime(state, |runtime| {
        runtime.rate_limits_available = rate_limits;
        runtime.account_usage_available = account_usage;
    });
}

fn update_capability_diagnostic(
    state: &AccountSensorState,
    kind: RequestKind,
    available: bool,
    reason: Option<String>,
) {
    update_runtime(state, |runtime| match kind {
        RequestKind::RateLimits => {
            runtime.rate_limits_available = Some(available);
            runtime.rate_limits_reason = reason;
        }
        RequestKind::AccountUsage => {
            runtime.account_usage_available = Some(available);
            runtime.account_usage_reason = reason;
        }
        RequestKind::Hooks => {
            runtime.hooks_inventory_available = Some(available);
            runtime.hooks_inventory_reason = reason;
        }
        RequestKind::Initialize => {}
    });
}

fn fail_hook_probe_if_current(
    state: &AccountSensorState,
    expected_generation: Option<u64>,
    reason: Option<String>,
) -> bool {
    let mut snapshot = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if expected_generation != Some(snapshot.hook_inventory_generation) {
        return false;
    }
    snapshot.permission_hook = None;
    snapshot.permission_hook_observed_at_ms = None;
    snapshot.runtime.hooks_inventory_available = Some(false);
    snapshot.runtime.hooks_inventory_reason = reason;
    snapshot.runtime.observed_at_ms = now_epoch_ms();
    true
}

fn store_and_emit_limits(app: &AppHandle, state: &AccountSensorState, snapshot: RateLimitSnapshot) {
    crate::tray_icon::sync_rate_limit_snapshot(app, &snapshot);
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .rate_limits = Some(snapshot.clone());
    let _ = app.emit("rate-limit-update", snapshot);
}

fn store_and_emit_usage(
    app: &AppHandle,
    state: &AccountSensorState,
    snapshot: AccountUsageSnapshot,
) {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .account_usage = Some(snapshot.clone());
    let _ = app.emit("account-usage-update", snapshot);
}

fn store_hook_probe(
    app: &AppHandle,
    state: &AccountSensorState,
    value: &Value,
    expected_generation: Option<u64>,
) {
    let expected_path = crate::permission::codex_install::hooks_path();
    let expected_path = expected_path.as_deref();
    let probe = value
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .flat_map(|entry| {
            entry
                .get("hooks")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .find_map(|hook| normalize_hook_probe(hook, expected_path));
    let observed_at_ms = now_epoch_ms();
    if !replace_hook_probe_if_current(state, probe.clone(), observed_at_ms, expected_generation) {
        return;
    }

    let activity = app
        .try_state::<crate::permission::PermissionActivityState>()
        .and_then(|activity| {
            activity
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&ID)
                .cloned()
        });
    let active =
        crate::permission::codex_install::activity_matches_probe(probe.as_ref(), activity.as_ref());
    let (status, detail) = hook_health(probe.as_ref(), active);
    emit_health(
        app,
        ID,
        ProviderComponent::Permissions,
        status,
        Some(detail),
    );
}

fn replace_hook_probe_if_current(
    state: &AccountSensorState,
    probe: Option<CodexHookProbe>,
    observed_at_ms: i64,
    expected_generation: Option<u64>,
) -> bool {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if expected_generation != Some(state.hook_inventory_generation) {
        return false;
    }
    state.permission_hook = probe;
    state.permission_hook_observed_at_ms = Some(observed_at_ms);
    state.runtime.hooks_inventory_available = Some(true);
    state.runtime.hooks_inventory_reason = None;
    state.runtime.observed_at_ms = observed_at_ms;
    true
}

pub(crate) fn clear_hook_probe(state: &AccountSensorState) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.permission_hook = None;
    state.permission_hook_observed_at_ms = None;
}

pub(crate) fn invalidate_hook_probe(state: &AccountSensorState) {
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.permission_hook = None;
    state.permission_hook_observed_at_ms = None;
    state.hook_inventory_generation = state.hook_inventory_generation.wrapping_add(1);
    state.runtime.hooks_inventory_available = None;
    state.runtime.hooks_inventory_reason = Some("hook configuration changed; probing again".into());
    state.runtime.observed_at_ms = now_epoch_ms();
}

fn mark_stale_if_expired(app: &AppHandle, state: &AccountSensorState) {
    let now = now_epoch_ms();
    let stale_after_ms = STALE_AFTER.as_millis() as i64;
    let (limits_expired, usage_expired) = {
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            state.rate_limits.as_ref().is_some_and(|snapshot| {
                now.saturating_sub(snapshot.observed_at_ms) >= stale_after_ms
            }),
            state.account_usage.as_ref().is_some_and(|snapshot| {
                now.saturating_sub(snapshot.observed_at_ms) >= stale_after_ms
            }),
        )
    };
    if limits_expired {
        mark_limits_quality(app, state, SourceQuality::Stale);
    }
    if usage_expired {
        mark_usage_quality(app, state, SourceQuality::Stale);
    }
}

fn mark_limits_quality(app: &AppHandle, state: &AccountSensorState, quality: SourceQuality) {
    let snapshot = {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(snapshot) = state.rate_limits.as_mut() else {
            return;
        };
        if snapshot.source_quality == quality
            || (quality == SourceQuality::Stale
                && snapshot.source_quality == SourceQuality::Unavailable)
        {
            return;
        }
        snapshot.source_quality = quality;
        snapshot.clone()
    };
    crate::tray_icon::sync_rate_limit_snapshot(app, &snapshot);
    let _ = app.emit("rate-limit-update", snapshot);
}

fn mark_usage_quality(app: &AppHandle, state: &AccountSensorState, quality: SourceQuality) {
    let snapshot = {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(snapshot) = state.account_usage.as_mut() else {
            return;
        };
        if snapshot.source_quality == quality
            || (quality == SourceQuality::Stale
                && snapshot.source_quality == SourceQuality::Unavailable)
        {
            return;
        }
        snapshot.source_quality = quality;
        snapshot.clone()
    };
    let _ = app.emit("account-usage-update", snapshot);
}

fn mark_unavailable_if_expired(app: &AppHandle, state: &AccountSensorState) {
    let now = now_epoch_ms();
    let (limits_expired, usage_expired) = {
        let state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            state.rate_limits.as_ref().is_some_and(|snapshot| {
                now.saturating_sub(snapshot.observed_at_ms) >= UNAVAILABLE_AFTER_MS
            }),
            state.account_usage.as_ref().is_some_and(|snapshot| {
                now.saturating_sub(snapshot.observed_at_ms) >= UNAVAILABLE_AFTER_MS
            }),
        )
    };
    if limits_expired {
        mark_limits_quality(app, state, SourceQuality::Unavailable);
    }
    if usage_expired {
        mark_usage_quality(app, state, SourceQuality::Unavailable);
    }
}

fn runtime_version(executable: &Path, path: Option<&str>) -> Option<String> {
    let mut command = Command::new(executable);
    command.arg("--version");
    if let Some(path) = path {
        command.env("PATH", path);
    }
    let output = command.output().ok()?;
    if !output.status.success() || output.stdout.len() > 4096 {
        return None;
    }
    let version = String::from_utf8(output.stdout).ok()?;
    let version = version.trim();
    (!version.is_empty()).then(|| version.to_string())
}

fn resolve_executable(binary: &str, path: Option<&str>) -> Option<PathBuf> {
    let owned;
    let path = match path {
        Some(path) => path,
        None => {
            owned = crate::env_lock::var_os("PATH")?;
            owned.to_str()?
        }
    };
    let extensions: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };
    for directory in std::env::split_paths(path) {
        for extension in extensions {
            let candidate = directory.join(format!("{binary}{extension}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hook_timeout_is_removed_without_expiring_account_requests() {
        let old = Instant::now() - Duration::from_secs(20);
        let fresh = Instant::now();
        let mut pending = HashMap::from([
            (
                1,
                PendingRequest {
                    kind: RequestKind::Hooks,
                    sent_at: old,
                    hook_inventory_generation: Some(0),
                },
            ),
            (
                2,
                PendingRequest {
                    kind: RequestKind::RateLimits,
                    sent_at: fresh,
                    hook_inventory_generation: None,
                },
            ),
        ]);
        assert_eq!(
            take_timed_out(&mut pending, Duration::from_secs(15))
                .into_iter()
                .map(|request| request.kind)
                .collect::<Vec<_>>(),
            vec![RequestKind::Hooks]
        );
        assert_eq!(
            pending.get(&2).map(|request| request.kind),
            Some(RequestKind::RateLimits)
        );

        let state = new_state();
        update_runtime(&state, |runtime| {
            runtime.connection_status = "connected".into();
            runtime.rate_limits_available = Some(true);
            runtime.account_usage_available = Some(true);
        });
        update_capability_diagnostic(
            &state,
            RequestKind::Hooks,
            false,
            Some("hooks/list timed out".into()),
        );
        let snapshot = state.lock().unwrap().clone();
        assert_eq!(snapshot.runtime.connection_status, "connected");
        assert_eq!(snapshot.runtime.rate_limits_available, Some(true));
        assert_eq!(snapshot.runtime.account_usage_available, Some(true));
        assert_eq!(snapshot.runtime.hooks_inventory_available, Some(false));
    }

    #[test]
    fn local_hook_mutation_invalidates_inventory_until_next_probe() {
        let state = new_state();
        let old_generation = state.lock().unwrap().hook_inventory_generation;
        update_runtime(&state, |runtime| {
            runtime.hooks_inventory_available = Some(true);
            runtime.hooks_inventory_reason = None;
        });
        invalidate_hook_probe(&state);
        let snapshot = state.lock().unwrap().clone();
        assert_eq!(snapshot.runtime.hooks_inventory_available, None);
        assert_eq!(
            snapshot.runtime.hooks_inventory_reason.as_deref(),
            Some("hook configuration changed; probing again")
        );
        assert!(snapshot.permission_hook.is_none());
        assert!(snapshot.permission_hook_observed_at_ms.is_none());
        drop(snapshot);

        let stale_probe = CodexHookProbe {
            enabled: true,
            trust_status: "trusted".into(),
            source_path: "/tmp/old-hooks.json".into(),
            current_hash: Some("old-hash".into()),
            observed_at_ms: 1,
        };
        assert!(!replace_hook_probe_if_current(
            &state,
            Some(stale_probe),
            2,
            Some(old_generation),
        ));
        let snapshot = state.lock().unwrap().clone();
        assert!(snapshot.permission_hook.is_none());
        assert_eq!(snapshot.runtime.hooks_inventory_available, None);
    }
}
