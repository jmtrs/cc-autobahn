//! Provider-neutral contracts. Adapters own provider-specific wire formats;
//! the rest of the application consumes these discriminated domain shapes.

pub mod claude;
pub mod codex;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

#[cfg(target_os = "macos")]
use std::io::Read;
#[cfg(target_os = "macos")]
use std::path::Path;
#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};
#[cfg(target_os = "macos")]
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// Starts every enabled provider adapter. The native shell calls this registry
/// once; adapters remain responsible for their own worker topology.
pub fn start_enabled(app: AppHandle) {
    claude::start(app.clone());
    codex::start(app);
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    #[default]
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceQuality {
    Official,
    Estimated,
    Local,
    Stale,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderComponent {
    Engine,
    Sensor,
    History,
    Transcript,
    Permissions,
    AppServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Connected,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderMarker {
    pub provider: ProviderId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub provider: ProviderId,
    pub component: ProviderComponent,
    pub status: HealthStatus,
    pub observed_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub provider: ProviderId,
    pub scope: String,
    pub observed_at_ms: i64,
    pub source_quality: SourceQuality,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
    pub started_at_ms: Option<i64>,
    #[serde(default)]
    pub model_breakdown: Vec<ModelUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRate {
    pub provider: ProviderId,
    pub source_quality: SourceQuality,
    pub session_or_thread_id: String,
    pub session_started_at_ms: Option<i64>,
    pub observed_at_ms: i64,
    pub output_tokens: u64,
    pub elapsed_ms: i64,
    pub tokens_per_second: f64,
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitSnapshot {
    pub provider: ProviderId,
    pub observed_at_ms: i64,
    pub source_quality: SourceQuality,
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<RateLimitBucket>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindow {
    pub used_percent: f64,
    pub window_duration_minutes: Option<u64>,
    pub resets_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitBucket {
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub plan_type: Option<String>,
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageSnapshot {
    pub provider: ProviderId,
    pub observed_at_ms: i64,
    pub source_quality: SourceQuality,
    pub lifetime_tokens: Option<u64>,
    pub peak_daily_tokens: Option<u64>,
    pub longest_running_turn_seconds: Option<u64>,
    pub current_streak_days: Option<u64>,
    pub longest_streak_days: Option<u64>,
    #[serde(default)]
    pub daily_usage: Vec<AccountDailyUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDailyUsage {
    pub start_date: String,
    pub tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelActivity {
    pub provider: ProviderId,
    pub model_id: String,
    pub session_or_thread_id: String,
    pub observed_at_ms: i64,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub provider: ProviderId,
    pub request_id: String,
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    pub turn_id: Option<String>,
    pub thread_id: Option<String>,
    pub item_id: Option<String>,
    pub tool_name: String,
    pub summary: String,
    pub cwd: String,
    pub received_at_ms: i64,
    pub expires_at_ms: i64,
    #[serde(default)]
    pub native_permission_suggestions: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDiagnostic {
    pub id: String,
    pub status: String,
    pub source: String,
    pub quality: String,
    pub fallback: Option<String>,
    pub reason: Option<String>,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDiagnostics {
    pub provider: ProviderId,
    pub surface: String,
    pub runtime_executable: Option<String>,
    pub runtime_version: Option<String>,
    pub related_runtimes: Vec<RuntimeDiagnostic>,
    pub compatibility: String,
    pub capabilities: Vec<CapabilityDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostic {
    pub surface: String,
    pub product_version: Option<String>,
    pub runtime_executable: Option<String>,
    pub runtime_version: Option<String>,
}

pub type ProviderHealthState = Arc<Mutex<HashMap<(ProviderId, ProviderComponent), ProviderHealth>>>;
pub type ProviderActivityState = Arc<Mutex<HashMap<ProviderId, ModelActivity>>>;

pub fn new_health_state() -> ProviderHealthState {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn new_activity_state() -> ProviderActivityState {
    Arc::new(Mutex::new(HashMap::new()))
}

fn record_activity(
    state: &ProviderActivityState,
    activity: ModelActivity,
) -> Option<ModelActivity> {
    let mut activities = state.lock().unwrap();
    let is_newer = activities.get(&activity.provider).is_none_or(|current| {
        if activity.observed_at_ms != current.observed_at_ms {
            return activity.observed_at_ms > current.observed_at_ms;
        }
        if activity.session_or_thread_id == current.session_or_thread_id {
            return activity.sequence > current.sequence;
        }
        (&activity.session_or_thread_id, &activity.model_id)
            > (&current.session_or_thread_id, &current.model_id)
    });
    if !is_newer {
        return None;
    }
    activities.insert(activity.provider, activity.clone());
    Some(activity)
}

fn record_health(state: &ProviderHealthState, mut health: ProviderHealth) -> ProviderHealth {
    let mut registry = state.lock().unwrap();
    if let Some(current) = registry.get(&(health.provider, health.component)) {
        health.observed_at_ms = health.observed_at_ms.max(current.observed_at_ms + 1);
    }
    registry.insert((health.provider, health.component), health.clone());
    health
}

fn health_snapshot(state: &ProviderHealthState) -> Vec<ProviderHealth> {
    let mut snapshot: Vec<_> = state.lock().unwrap().values().cloned().collect();
    snapshot.sort_by_key(|health| (health.provider, health.component));
    snapshot
}

#[tauri::command]
pub fn provider_health_snapshot(app: AppHandle) -> Vec<ProviderHealth> {
    let state = app.state::<ProviderHealthState>();
    health_snapshot(&state)
}

#[tauri::command]
pub fn provider_activity_snapshot(app: AppHandle) -> Vec<ModelActivity> {
    let mut snapshot: Vec<_> = app
        .state::<ProviderActivityState>()
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect();
    snapshot.sort_by_key(|activity| (activity.observed_at_ms, activity.sequence));
    snapshot
}

#[tauri::command]
pub fn provider_diagnostics_snapshot(app: AppHandle) -> Vec<ProviderDiagnostics> {
    let health = app
        .state::<ProviderHealthState>()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let codex = app
        .state::<codex::app_server::AccountSensorState>()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let activity = app
        .try_state::<crate::permission::PermissionActivityState>()
        .and_then(|state| {
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&ProviderId::Codex)
                .cloned()
        });
    let hook_active = crate::permission::codex_install::activity_matches_probe(
        codex.permission_hook.as_ref(),
        activity.as_ref(),
    );
    build_provider_diagnostics(&health, codex, hook_active, related_codex_runtimes())
}

fn build_provider_diagnostics(
    health: &HashMap<(ProviderId, ProviderComponent), ProviderHealth>,
    codex: codex::app_server::AccountSensorSnapshot,
    hook_active: bool,
    related_codex_runtimes: Vec<RuntimeDiagnostic>,
) -> Vec<ProviderDiagnostics> {
    let component_status =
        |provider, component| health.get(&(provider, component)).map(|value| value.status);
    let health_capability = |provider: ProviderId,
                             component: ProviderComponent,
                             id: &str,
                             source: &str,
                             quality: &str,
                             remediation: &str| {
        let status = component_status(provider, component);
        CapabilityDiagnostic {
            id: id.into(),
            status: match status {
                Some(HealthStatus::Connected) => "available",
                Some(HealthStatus::Degraded) => "degraded",
                Some(HealthStatus::Unavailable) => "unavailable",
                None => "unverified",
            }
            .into(),
            source: source.into(),
            quality: quality.into(),
            fallback: None,
            reason: health
                .get(&(provider, component))
                .and_then(|value| value.detail.clone()),
            remediation: (status != Some(HealthStatus::Connected)).then(|| remediation.to_string()),
        }
    };

    let claude_capabilities = vec![
        health_capability(
            ProviderId::Claude,
            ProviderComponent::Engine,
            "usage-history",
            "ccusage claude",
            "estimated",
            "Install or reconnect ccusage from Settings.",
        ),
        health_capability(
            ProviderId::Claude,
            ProviderComponent::Sensor,
            "limits",
            "Claude statusLine",
            "official",
            "Connect the Claude statusLine sensor.",
        ),
        health_capability(
            ProviderId::Claude,
            ProviderComponent::Transcript,
            "live-activity",
            "Claude transcript",
            "local",
            "Start a Claude Code session with transcript access.",
        ),
        health_capability(
            ProviderId::Claude,
            ProviderComponent::Permissions,
            "permissions",
            "Claude PermissionRequest hook",
            "native",
            "Connect the Claude permission hook from Settings.",
        ),
    ];
    let bool_capability = |id: &str,
                           available: Option<bool>,
                           source: &str,
                           quality: &str,
                           fallback: Option<&str>,
                           reason: Option<&str>,
                           remediation: &str| CapabilityDiagnostic {
        id: id.into(),
        status: match available {
            Some(true) => "available",
            Some(false) => "unavailable",
            None => "unverified",
        }
        .into(),
        source: source.into(),
        quality: quality.into(),
        fallback: fallback.map(str::to_string),
        reason: reason.map(str::to_string),
        remediation: (available != Some(true)).then(|| remediation.to_string()),
    };
    let transcript = component_status(ProviderId::Codex, ProviderComponent::Transcript);
    let history = component_status(ProviderId::Codex, ProviderComponent::History);
    let local_fallback_available = matches!(
        (transcript, history),
        (Some(HealthStatus::Connected), _) | (_, Some(HealthStatus::Connected))
    );
    let mut codex_capabilities = vec![
        bool_capability(
            "limits",
            codex.runtime.rate_limits_available,
            "Codex App Server",
            "official",
            None,
            codex.runtime.rate_limits_reason.as_deref(),
            "Use a compatible ChatGPT-authenticated Codex runtime.",
        ),
        bool_capability(
            "account-usage",
            codex.runtime.account_usage_available,
            "Codex App Server",
            "official",
            (history == Some(HealthStatus::Connected)).then_some("ccusage codex"),
            codex.runtime.account_usage_reason.as_deref(),
            "Use ChatGPT authentication or rely on estimated local history.",
        ),
        bool_capability(
            "hook-inventory",
            codex.runtime.hooks_inventory_available,
            "hooks/list",
            "native",
            None,
            codex.runtime.hooks_inventory_reason.as_deref(),
            "Open /hooks in Codex and review the user hook.",
        ),
    ];
    codex_capabilities.push(connection_capability(
        &codex.runtime,
        local_fallback_available,
    ));
    codex_capabilities.extend(permission_diagnostics(&codex, hook_active));
    codex_capabilities.push(CapabilityDiagnostic {
        id: "live-activity".into(),
        status: match transcript {
            Some(HealthStatus::Connected) => "available",
            Some(HealthStatus::Degraded) => "degraded",
            Some(HealthStatus::Unavailable) => "unavailable",
            None => "unverified",
        }
        .into(),
        source: "Codex rollout".into(),
        quality: "local".into(),
        fallback: None,
        reason: health
            .get(&(ProviderId::Codex, ProviderComponent::Transcript))
            .and_then(|value| value.detail.clone()),
        remediation: (transcript != Some(HealthStatus::Connected))
            .then(|| "Start a Codex session with a recognized local rollout.".into()),
    });
    let mut history_capability = health_capability(
        ProviderId::Codex,
        ProviderComponent::History,
        "history",
        "ccusage codex",
        "estimated",
        "Install or reconnect ccusage from Settings.",
    );
    if history == Some(HealthStatus::Degraded) {
        history_capability.status = "unavailable".into();
    }
    codex_capabilities.push(history_capability);
    let claude_compatibility = compatibility_for(&claude_capabilities);
    let codex_compatibility = compatibility_for(&codex_capabilities);

    vec![
        ProviderDiagnostics {
            provider: ProviderId::Claude,
            surface: "Claude Code · external sessions".into(),
            runtime_executable: None,
            runtime_version: None,
            related_runtimes: Vec::new(),
            compatibility: claude_compatibility.into(),
            capabilities: claude_capabilities,
        },
        ProviderDiagnostics {
            provider: ProviderId::Codex,
            surface: "Codex CLI · selected App Server runtime; ChatGPT desktop is independent"
                .into(),
            runtime_executable: codex.runtime.executable_path,
            runtime_version: codex.runtime.version,
            related_runtimes: related_codex_runtimes,
            compatibility: codex_compatibility.into(),
            capabilities: codex_capabilities,
        },
    ]
}

fn compatibility_for(capabilities: &[CapabilityDiagnostic]) -> &'static str {
    let relevant: Vec<_> = capabilities
        .iter()
        .filter(|capability| capability.id != "native-approval-fallback")
        .collect();
    if !relevant.is_empty()
        && relevant
            .iter()
            .all(|capability| capability.status == "available")
    {
        "compatible"
    } else if relevant.iter().any(|capability| {
        matches!(capability.status.as_str(), "available" | "degraded")
            || (capability.fallback.is_some() && !capability.id.starts_with("permission-hook-"))
    }) {
        "partial"
    } else {
        "unsupported"
    }
}

fn connection_capability(
    runtime: &codex::app_server::CodexRuntimeDiagnostics,
    local_fallback_available: bool,
) -> CapabilityDiagnostic {
    let connected = runtime.connection_status == "connected";
    CapabilityDiagnostic {
        id: "app-server-connection".into(),
        status: if connected {
            "available"
        } else if runtime.connection_status == "connecting" {
            "unverified"
        } else {
            "unavailable"
        }
        .into(),
        source: runtime
            .executable_path
            .clone()
            .unwrap_or_else(|| "selected Codex executable".into()),
        quality: "official".into(),
        fallback: local_fallback_available.then(|| "local rollout and ccusage".into()),
        reason: (!connected).then(|| runtime.connection_status.clone()),
        remediation: (!connected)
            .then(|| "Select a working Codex executable and verify authentication.".into()),
    }
}

fn permission_diagnostics(
    codex: &codex::app_server::AccountSensorSnapshot,
    hook_active: bool,
) -> Vec<CapabilityDiagnostic> {
    let probe = codex.permission_hook.as_ref();
    let inventoried = codex.runtime.hooks_inventory_available == Some(true);
    let state = |id: &str, available: bool, reason: String| CapabilityDiagnostic {
        id: id.into(),
        status: if available {
            "available"
        } else if inventoried {
            "unavailable"
        } else {
            "unverified"
        }
        .into(),
        source: probe
            .map(|probe| probe.source_path.clone())
            .unwrap_or_else(|| "hooks/list".into()),
        quality: "native".into(),
        fallback: Some("Codex native approval UI".into()),
        reason: (!available).then_some(reason),
        remediation: (!available).then(|| "Review the hook in Codex /hooks.".into()),
    };
    vec![
        state(
            "permission-hook-installed",
            probe.is_some(),
            "cc-autobahn hook not found in the inventoried configuration".into(),
        ),
        state(
            "permission-hook-enabled",
            probe.is_some_and(|probe| probe.enabled),
            "hook is disabled or not inventoried".into(),
        ),
        state(
            "permission-hook-trusted",
            probe.is_some_and(|probe| probe.trust_status == "trusted"),
            probe.map_or_else(
                || "hook trust is unverified".into(),
                |probe| format!("hook trust is {}", probe.trust_status),
            ),
        ),
        state(
            "permission-hook-active",
            hook_active,
            "no successful exchange for the current trusted hook hash".into(),
        ),
        CapabilityDiagnostic {
            id: "native-approval-fallback".into(),
            status: "available".into(),
            source: "Codex native approval UI".into(),
            quality: "native".into(),
            fallback: None,
            reason: None,
            remediation: None,
        },
    ]
}

#[cfg(target_os = "macos")]
fn related_codex_runtimes() -> Vec<RuntimeDiagnostic> {
    [
        ("ChatGPT desktop", "/Applications/ChatGPT.app"),
        ("Codex desktop (compatibility)", "/Applications/Codex.app"),
    ]
    .into_iter()
    .filter_map(|(surface, bundle)| {
        let bundle = Path::new(bundle);
        bundle.is_dir().then(|| {
            let executable = bundle.join("Contents/Resources/codex");
            RuntimeDiagnostic {
                surface: surface.into(),
                product_version: bundle_product_version(bundle),
                runtime_executable: executable
                    .is_file()
                    .then(|| executable.to_string_lossy().into_owned()),
                runtime_version: executable
                    .is_file()
                    .then(|| command_version(&executable))
                    .flatten(),
            }
        })
    })
    .collect()
}

#[cfg(not(target_os = "macos"))]
fn related_codex_runtimes() -> Vec<RuntimeDiagnostic> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn bundle_product_version(bundle: &Path) -> Option<String> {
    let info = bundle.join("Contents/Info.plist");
    let mut command = Command::new("/usr/bin/plutil");
    command
        .args(["-extract", "CFBundleShortVersionString", "raw", "-o", "-"])
        .arg(info);
    bounded_stdout(&mut command, Duration::from_secs(2)).and_then(|bytes| output_text(&bytes))
}

#[cfg(target_os = "macos")]
fn command_version(executable: &Path) -> Option<String> {
    let mut command = Command::new(executable);
    command.arg("--version");
    bounded_stdout(&mut command, Duration::from_secs(2)).and_then(|bytes| output_text(&bytes))
}

#[cfg(target_os = "macos")]
fn bounded_stdout(command: &mut Command, timeout: Duration) -> Option<Vec<u8>> {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    if !status.success() {
        return None;
    }
    let mut bytes = Vec::new();
    child.stdout.take()?.read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

#[cfg(target_os = "macos")]
fn output_text(bytes: &[u8]) -> Option<String> {
    let value = String::from_utf8_lossy(bytes).trim().to_string();
    (!value.is_empty()).then_some(value)
}

pub fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

pub fn emit_health(
    app: &AppHandle,
    provider: ProviderId,
    component: ProviderComponent,
    status: HealthStatus,
    detail: Option<String>,
) {
    let health = ProviderHealth {
        provider,
        component,
        status,
        observed_at_ms: now_epoch_ms(),
        detail,
    };
    let state = app.state::<ProviderHealthState>();
    let health = record_health(&state, health);
    let _ = app.emit("provider-health", health);
}

pub fn emit_model_activity(app: &AppHandle, activity: ModelActivity) {
    let state = app.state::<ProviderActivityState>();
    if let Some(activity) = record_activity(&state, activity) {
        let _ = app.emit("model-activity", activity);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_id_serializes_as_stable_lowercase_discriminator() {
        assert_eq!(serde_json::to_value(ProviderId::Claude).unwrap(), "claude");
        assert_eq!(serde_json::to_value(ProviderId::Codex).unwrap(), "codex");
    }

    #[test]
    fn provider_health_keeps_component_failure_local() {
        let value = serde_json::to_value(ProviderHealth {
            provider: ProviderId::Codex,
            component: ProviderComponent::AppServer,
            status: HealthStatus::Degraded,
            observed_at_ms: 42,
            detail: Some("capability unavailable".into()),
        })
        .unwrap();
        assert_eq!(value["provider"], "codex");
        assert_eq!(value["component"], "app-server");
        assert_eq!(value["status"], "degraded");
        assert_eq!(value["observedAtMs"], 42);
    }

    #[test]
    fn normalized_rate_limit_preserves_source_quality() {
        let snapshot = RateLimitSnapshot {
            provider: ProviderId::Claude,
            observed_at_ms: 42,
            source_quality: SourceQuality::Official,
            primary: Some(RateLimitWindow {
                used_percent: 25.0,
                window_duration_minutes: Some(300),
                resets_at_ms: Some(1000),
            }),
            secondary: None,
            buckets: vec![],
        };
        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["sourceQuality"], "official");
        assert_eq!(value["primary"]["windowDurationMinutes"], 300);
        assert!(value["secondary"].is_null());
    }

    #[test]
    fn local_turn_rate_serializes_provenance() {
        let value = serde_json::to_value(TurnRate {
            provider: ProviderId::Codex,
            source_quality: SourceQuality::Local,
            session_or_thread_id: "thread-1".into(),
            session_started_at_ms: Some(1),
            observed_at_ms: 42,
            output_tokens: 10,
            elapsed_ms: 1_000,
            tokens_per_second: 10.0,
            partial: false,
        })
        .unwrap();
        assert_eq!(value["sourceQuality"], "local");
        assert_eq!(value["sessionStartedAtMs"], 1);
    }

    #[test]
    fn diagnostics_do_not_claim_unobserved_history_and_keep_runtime_identity() {
        let mut health = HashMap::new();
        health.insert(
            (ProviderId::Codex, ProviderComponent::Transcript),
            ProviderHealth {
                provider: ProviderId::Codex,
                component: ProviderComponent::Transcript,
                status: HealthStatus::Connected,
                observed_at_ms: 10,
                detail: None,
            },
        );
        let mut codex = codex::app_server::AccountSensorSnapshot::default();
        codex.runtime.executable_path = Some("/opt/codex".into());
        codex.runtime.version = Some("codex 1.2.3".into());
        codex.runtime.rate_limits_available = Some(false);

        let diagnostics = build_provider_diagnostics(&health, codex, false, Vec::new());
        let codex = diagnostics
            .iter()
            .find(|provider| provider.provider == ProviderId::Codex)
            .unwrap();
        assert_eq!(codex.runtime_executable.as_deref(), Some("/opt/codex"));
        assert_eq!(codex.compatibility, "partial");
        assert_eq!(
            codex
                .capabilities
                .iter()
                .find(|capability| capability.id == "history")
                .unwrap()
                .status,
            "unverified"
        );
    }

    #[test]
    fn compatibility_requires_every_relevant_capability_and_rejects_metadata_only() {
        let capability = |status: &str, fallback: Option<&str>| CapabilityDiagnostic {
            id: "test".into(),
            status: status.into(),
            source: "fixture".into(),
            quality: "official".into(),
            fallback: fallback.map(str::to_string),
            reason: None,
            remediation: None,
        };
        assert_eq!(
            compatibility_for(&[capability("available", None)]),
            "compatible"
        );
        assert_eq!(
            compatibility_for(&[
                capability("available", None),
                capability("unavailable", Some("local")),
            ]),
            "partial"
        );
        assert_eq!(
            compatibility_for(&[capability("unverified", None)]),
            "unsupported"
        );
        let mut permission_fallback = capability("unverified", Some("native UI"));
        permission_fallback.id = "permission-hook-active".into();
        assert_eq!(compatibility_for(&[permission_fallback]), "unsupported");
    }

    #[test]
    fn empty_codex_snapshot_is_unsupported_until_a_source_is_observed() {
        let diagnostics = build_provider_diagnostics(
            &HashMap::new(),
            codex::app_server::AccountSensorSnapshot::default(),
            false,
            Vec::new(),
        );
        let codex = diagnostics
            .iter()
            .find(|provider| provider.provider == ProviderId::Codex)
            .unwrap();
        assert_eq!(codex.compatibility, "unsupported");
    }

    #[test]
    fn failed_history_probe_does_not_count_as_an_observed_fallback() {
        let health = HashMap::from([(
            (ProviderId::Codex, ProviderComponent::History),
            ProviderHealth {
                provider: ProviderId::Codex,
                component: ProviderComponent::History,
                status: HealthStatus::Degraded,
                observed_at_ms: 10,
                detail: Some("ccusage failed".into()),
            },
        )]);
        let diagnostics = build_provider_diagnostics(
            &health,
            codex::app_server::AccountSensorSnapshot::default(),
            false,
            Vec::new(),
        );
        let codex = diagnostics
            .iter()
            .find(|provider| provider.provider == ProviderId::Codex)
            .unwrap();
        assert!(codex
            .capabilities
            .iter()
            .find(|capability| capability.id == "account-usage")
            .unwrap()
            .fallback
            .is_none());
        assert_eq!(codex.compatibility, "unsupported");
    }

    #[test]
    fn related_runtime_serializes_as_a_distinct_surface() {
        let value = serde_json::to_value(RuntimeDiagnostic {
            surface: "ChatGPT desktop".into(),
            product_version: Some("26.715.31925".into()),
            runtime_executable: Some("/Applications/ChatGPT.app/Contents/Resources/codex".into()),
            runtime_version: Some("codex-cli 0.145.0-alpha.18".into()),
        })
        .unwrap();
        assert_eq!(value["surface"], "ChatGPT desktop");
        assert_eq!(value["productVersion"], "26.715.31925");
        assert_eq!(value["runtimeVersion"], "codex-cli 0.145.0-alpha.18");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn bundled_runtime_probe_has_a_hard_deadline() {
        let mut command = Command::new("/bin/sleep");
        command.arg("1");
        let started = Instant::now();
        assert!(bounded_stdout(&mut command, Duration::from_millis(20)).is_none());
        assert!(started.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn health_emitted_before_frontend_subscription_remains_in_snapshot() {
        let state = new_health_state();
        record_health(
            &state,
            ProviderHealth {
                provider: ProviderId::Claude,
                component: ProviderComponent::Engine,
                status: HealthStatus::Connected,
                observed_at_ms: 10,
                detail: None,
            },
        );
        record_health(
            &state,
            ProviderHealth {
                provider: ProviderId::Claude,
                component: ProviderComponent::Engine,
                status: HealthStatus::Degraded,
                observed_at_ms: 5,
                detail: Some("poll failed".into()),
            },
        );

        let snapshot = health_snapshot(&state);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].status, HealthStatus::Degraded);
        assert_eq!(snapshot[0].observed_at_ms, 11);
    }

    #[test]
    fn activity_registry_rejects_delayed_provider_events() {
        let state = new_activity_state();
        let newer = ModelActivity {
            provider: ProviderId::Codex,
            model_id: "gpt-new".into(),
            session_or_thread_id: "thread-new".into(),
            observed_at_ms: 200,
            sequence: 1,
        };
        assert_eq!(record_activity(&state, newer.clone()), Some(newer.clone()));
        let delayed = ModelActivity {
            provider: ProviderId::Codex,
            model_id: "gpt-old".into(),
            session_or_thread_id: "thread-old".into(),
            observed_at_ms: 199,
            sequence: 99,
        };
        assert_eq!(record_activity(&state, delayed), None);
        assert_eq!(state.lock().unwrap().get(&ProviderId::Codex), Some(&newer));
    }

    #[test]
    fn activity_registry_uses_a_stable_cross_session_tie_break() {
        let state = new_activity_state();
        let root = ModelActivity {
            provider: ProviderId::Codex,
            model_id: "gpt-root".into(),
            session_or_thread_id: "thread-z".into(),
            observed_at_ms: 200,
            sequence: 1,
        };
        let subagent = ModelActivity {
            provider: ProviderId::Codex,
            model_id: "gpt-subagent".into(),
            session_or_thread_id: "thread-a".into(),
            observed_at_ms: 200,
            sequence: 99,
        };
        assert_eq!(
            record_activity(&state, subagent),
            Some(ModelActivity {
                provider: ProviderId::Codex,
                model_id: "gpt-subagent".into(),
                session_or_thread_id: "thread-a".into(),
                observed_at_ms: 200,
                sequence: 99,
            })
        );
        assert_eq!(record_activity(&state, root.clone()), Some(root.clone()));
        assert_eq!(state.lock().unwrap().get(&ProviderId::Codex), Some(&root));
    }
}
