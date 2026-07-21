//! permission — self-installed `PermissionRequest` hook (D42): the cluster
//! becomes the approve/deny surface for ANY Claude Code session's permission
//! prompts, instead of alt-tabbing to a terminal.
//!
//! Unlike the statusLine sensor (D12), which is fire-and-forget (a short-lived
//! CLI process writes a file, the GUI polls it later at its own leisure), a
//! Claude Code hook is SYNCHRONOUS — it blocks CC until the hook process
//! exits, so a hook that just writes a file and exits can't wait for a human
//! to click a button. This needs a real request/response round trip, so it
//! gets a real IPC primitive: a Unix domain socket (`std::os::unix::net`,
//! stdlib, zero new deps — D16 spirit) instead of the file+poll pattern.
//!
//! `hook_bin::run_permission_hook` (invoked as `cc-autobahn permission-hook`,
//! same dual-binary dispatch as `sensor::run_statusline`, D19) is the CLI
//! side: it connects, sends one request line, blocks on a read with a
//! timeout, and prints Claude Code's expected decision JSON — or, on ANY
//! failure (no socket, timeout, malformed input), prints NOTHING and exits 0.
//! That silence is the entire fail-open mechanism: no valid JSON on stdout is
//! already Claude Code's own signal to fall back to its normal terminal
//! prompt. A dead cc-autobahn must never hang a real coding session.
//!
//! This module is the GUI side: a dedicated thread (same "one thread per
//! concern" shape as `engine`/`burn`/`sensor`) accepts connections, queues
//! concurrent requests FIFO (`PendingQueue`), and exposes `permission_approve`/
//! `permission_deny` commands that pop a request by id and reply through its
//! own channel — never assuming the queue's front is still the request being
//! resolved, since a timeout can drop an entry out from under a stale click.

pub mod always_allow;
pub mod codex_install;
pub mod hook_bin;
pub mod install;
mod transport;

use always_allow::AlwaysAllowSet;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// `${TMPDIR}/.cc-autobahn/permission.sock` — provider-neutral
/// request/response bridge between a blocking hook process and this GUI.
///
/// Codex/OWL sandboxes may deny Unix-socket connections under `$HOME` even
/// when the hook command itself is trusted. Their private temporary directory
/// remains available to local tools, so prefer it and retain the historical
/// home-directory location only as a fallback for environments without
/// `TMPDIR`.
pub(crate) fn socket_path() -> Option<PathBuf> {
    socket_path_from(
        crate::env_lock::var_os("TMPDIR"),
        crate::env_lock::var_os("HOME"),
    )
}

/// Every socket location the running GUI could plausibly have bound, in the
/// same priority order [`socket_path`] would pick — TMPDIR first, HOME as
/// fallback. A single hook invocation and the long-running GUI process can
/// see different environments (e.g. one launched from a terminal, the other
/// from Finder/a login item), so a hook computing only [`socket_path`] can
/// silently talk to an empty directory while the GUI listens on the other
/// one. The hook client probes both instead of trusting a single guess; the
/// GUI itself still binds exactly one (no ambiguity on the server side).
pub(crate) fn socket_path_candidates() -> Vec<PathBuf> {
    socket_path_candidates_from(
        crate::env_lock::var_os("TMPDIR"),
        crate::env_lock::var_os("HOME"),
    )
}

fn socket_path_candidates_from(
    tmpdir: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = socket_path_from(tmpdir, None) {
        candidates.push(path);
    }
    if let Some(path) = socket_path_from(None, home) {
        if !candidates.contains(&path) {
            candidates.push(path);
        }
    }
    candidates
}

fn socket_path_from(
    tmpdir: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let root = tmpdir
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .or_else(|| home.map(PathBuf::from))?;
    Some(root.join(".cc-autobahn").join("permission.sock"))
}

fn file_bridge_dirs() -> Option<(PathBuf, PathBuf)> {
    file_bridge_dirs_at(&socket_path()?)
}

pub(crate) fn file_bridge_dirs_at(socket_path: &std::path::Path) -> Option<(PathBuf, PathBuf)> {
    let root = socket_path.parent()?.to_path_buf();
    Some((root.join("requests"), root.join("responses")))
}

fn safe_request_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

// ─────────────────────────────────────────────────────────────────────────────
// Wire types (socket protocol, one JSON line each way)
// ─────────────────────────────────────────────────────────────────────────────

/// Hook → GUI, one line. Deliberately narrow (D-review: no speculative
/// fields) — just enough to render a decision prompt, not the full stdin
/// payload Claude Code hands the hook (`transcript_path`/`permission_mode`
/// aren't forwarded).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HookRequest {
    #[serde(default)]
    pub(crate) provider: crate::providers::ProviderId,
    /// Unique identity generated by this individual hook process. Claude's
    /// `prompt_id` correlates work to a user prompt and is neither required
    /// nor unique per permission request.
    pub(crate) request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prompt_id: Option<String>,
    pub(crate) session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) permission_mode: Option<String>,
    pub(crate) tool_name: String,
    #[serde(default)]
    pub(crate) tool_input: serde_json::Value,
    #[serde(default)]
    pub(crate) cwd: String,
    /// Provider-authored "always allow" choices. Kept as opaque JSON so the
    /// hook can echo an exact supported suggestion without duplicating or
    /// weakening Claude's permission-policy schema.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(crate) permission_suggestions: Vec<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Decision {
    Allow,
    Deny,
}

/// GUI → hook, one line. `updated_permissions` is present only when the
/// user selected a provider-native Always Allow suggestion.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DecisionMsg {
    decision: Decision,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    updated_permissions: Vec<serde_json::Value>,
}

impl DecisionMsg {
    fn plain(decision: Decision) -> Self {
        Self {
            decision,
            updated_permissions: Vec::new(),
        }
    }

    fn allow_with_permissions(updated_permissions: Vec<serde_json::Value>) -> Self {
        Self {
            decision: Decision::Allow,
            updated_permissions,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// FIFO queue (shared Tauri state)
// ─────────────────────────────────────────────────────────────────────────────

struct PendingSlot {
    request: HookRequest,
    reply_tx: Sender<DecisionMsg>,
    /// Derived once at queue time (cheap, human-paced frequency): the
    /// context row of the gate card — project name + git branch of the
    /// session's cwd. Not recomputed on every `emit_state`.
    project: String,
    branch: Option<String>,
    /// Wall-clock deadline this entry gives up at (D-review: mirrors
    /// `QUEUE_TIMEOUT_SECS`, set once at queue time) — surfaced to the
    /// frontend as `expiresAtMs` so the gate card can show a countdown
    /// instead of looking permanently stuck when the underlying hook
    /// process gets orphaned (Claude Code resolved the permission through
    /// some other path with no way to tell our hook it's moot).
    expires_at: std::time::SystemTime,
}

type PendingQueue = Arc<Mutex<VecDeque<PendingSlot>>>;
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PermissionActivity {
    pub(crate) observed_at_ms: i64,
    pub(crate) hook_hash: Option<String>,
}

pub(crate) type PermissionActivityState =
    Arc<Mutex<std::collections::HashMap<crate::providers::ProviderId, PermissionActivity>>>;

/// Serializes queue snapshots with their tray/frontend side effects. Queue
/// mutations release `PendingQueue` before calling [`emit_state`], so without
/// this a delayed `resolved` emission can overtake a newer `pending` emission
/// and leave the UI hidden while the queue is non-empty.
type PermissionEmissionLock = Arc<Mutex<()>>;

/// Payload of the `permission-pending` event — always the current head of the
/// queue plus a count, whether this arrival became the head or just landed
/// behind an existing one (the frontend re-renders from the payload, it
/// doesn't diff its own state).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingPayload {
    provider: crate::providers::ProviderId,
    id: String,
    tool_name: String,
    tool_input_summary: String,
    cwd: String,
    project: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    branch: Option<String>,
    pending_count: usize,
    provider_pending_count: usize,
    always_allow_available: bool,
    expires_at_ms: i64,
}

impl PendingPayload {
    fn from_slot(slot: &PendingSlot, pending_count: usize, provider_pending_count: usize) -> Self {
        PendingPayload {
            provider: slot.request.provider,
            id: slot.request.request_id.clone(),
            tool_name: slot.request.tool_name.clone(),
            tool_input_summary: summarize_tool_input(
                &slot.request.tool_name,
                &slot.request.tool_input,
            ),
            cwd: slot.request.cwd.clone(),
            project: slot.project.clone(),
            branch: slot.branch.clone(),
            pending_count,
            provider_pending_count,
            always_allow_available: match slot.request.provider {
                crate::providers::ProviderId::Codex => true,
                crate::providers::ProviderId::Claude => {
                    slot.request.permission_suggestions.len() == 1
                        || (slot.request.permission_suggestions.is_empty()
                            && matched_field(&slot.request.tool_name, &slot.request.tool_input)
                                .is_some())
                }
            },
            expires_at_ms: slot
                .expires_at
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0),
        }
    }
}

/// Project name for the gate card's context row: the basename of the
/// session's cwd (`/code/web-app` → `web-app`). Falls back to the raw cwd
/// when it has no basename (`/`), and to `?` for an empty cwd (a hook
/// payload without one — nothing better to show). PURE → testable.
fn cwd_project(cwd: &str) -> String {
    if cwd.is_empty() {
        return "?".to_string();
    }
    std::path::Path::new(cwd)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| cwd.to_string())
}

/// Current git branch of the session's cwd, for the context row. `None` on
/// any failure (not a repo, detached handling aside, git missing, io error)
/// — the card simply shows the project alone. Impure (spawns a process),
/// same D16 `std::process::Command` pattern as `always_allow::git_toplevel`.
fn git_branch(cwd: &str) -> Option<String> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let branch = String::from_utf8(out.stdout).ok()?.trim().to_string();
    if branch.is_empty() {
        None
    } else {
        Some(branch)
    }
}

/// The exact `tool_input` field cc-autobahn treats as "the thing a human
/// needs to see/approve" for a given tool — the single source of truth for
/// both the gate panel's display AND which tools support "Always Allow"
/// (`always_allow.rs`): only a tool with a mapped field has a safe, specific
/// rule to build. `None` for anything unmapped (no display truncation issue
/// there either — `summarize_tool_input` falls back to the raw JSON).
pub(crate) fn matched_field(tool_name: &str, input: &serde_json::Value) -> Option<String> {
    let field = match tool_name {
        "Bash" => "command",
        "Write" | "Edit" | "Read" => "file_path",
        _ => return None,
    };
    input
        .get(field)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

/// Tool-aware summary of `tool_input` for the gate panel. No truncation —
/// `.sensor-body`'s CSS already scrolls/wraps a bounded panel, and
/// `MAX_REQUEST_BYTES` already bounds worst-case size at the wire layer, so
/// cutting the text here only threw away information a human needs to
/// actually decide whether to approve. Tools with no mapped field (below or
/// in `matched_field`) fall back to the raw JSON — acceptable for a flat
/// object, but `AskUserQuestion`'s nested `questions[]` shape is unreadable
/// that way, hence its own case.
fn summarize_tool_input(tool_name: &str, input: &serde_json::Value) -> String {
    let string_field = match tool_name {
        "Bash" => input.get("command"),
        "apply_patch" | "Edit" | "Write" => input
            .get("patch")
            .or_else(|| input.get("file_path"))
            .or_else(|| input.get("path")),
        _ => input
            .get("command")
            .or_else(|| input.get("query"))
            .or_else(|| input.get("path"))
            .or_else(|| input.get("url")),
    }
    .and_then(|value| value.as_str())
    .map(str::to_string);

    string_field
        .or_else(|| {
            (tool_name == "AskUserQuestion")
                .then(|| summarize_questions(input))
                .flatten()
        })
        .unwrap_or_else(|| input.to_string())
}

/// First question's text, with a `(+N more)` suffix when the tool asked
/// several at once — matches how the gate already shows one thing to decide
/// on plus a pending count, not the full nested `questions[]` array.
fn summarize_questions(input: &serde_json::Value) -> Option<String> {
    let questions = input.get("questions")?.as_array()?;
    let first = questions.first()?.get("question")?.as_str()?;
    Some(match questions.len() {
        1 => first.to_string(),
        n => format!("{first} (+{} more)", n - 1),
    })
}

fn local_always_allow_match(request: &HookRequest) -> Option<String> {
    match request.provider {
        crate::providers::ProviderId::Codex => serde_json::to_string(&request.tool_input).ok(),
        crate::providers::ProviderId::Claude => request
            .permission_suggestions
            .is_empty()
            .then(|| matched_field(&request.tool_name, &request.tool_input))
            .flatten(),
    }
}

/// Emits `permission-pending` (with the current head + count) or
/// `permission-resolved` (queue empty), and mirrors the same state onto the
/// tray ring (`tray_icon::set_permission_pending`) — single place that keeps
/// the frontend event and the tray badge in sync, called after every queue
/// mutation (push, approve/deny, timeout-drop).
fn emit_state(app: &AppHandle, queue: &PendingQueue) {
    let emission_lock = app.state::<PermissionEmissionLock>();
    let _emission_guard = emission_lock.lock().unwrap();
    let payload = {
        let q = queue.lock().unwrap();
        q.front().map(|slot| {
            let provider_pending_count = q
                .iter()
                .filter(|pending| pending.request.provider == slot.request.provider)
                .count();
            PendingPayload::from_slot(slot, q.len(), provider_pending_count)
        })
    };
    match payload {
        Some(p) => {
            crate::tray_icon::set_permission_pending(app, true);
            // A request that arrives while the panel is hidden (the common
            // case — hide-on-blur means it usually is) would otherwise only
            // ever show up as a blinking tray icon, defeating the point of
            // approving without alt-tabbing to a terminal.
            crate::window::show_for_permission(app);
            let _ = app.emit("permission-pending", p);
        }
        None => {
            crate::tray_icon::set_permission_pending(app, false);
            let _ = app.emit(
                "permission-resolved",
                crate::providers::ProviderMarker {
                    // Queue-empty is global. Frontend hides the one global
                    // gate regardless of which provider resolved last.
                    provider: crate::providers::ProviderId::Claude,
                },
            );
            // Only auto-close once NOTHING permission-related is pending —
            // the Codex desktop mirror (rollout.rs) is a separate map, not
            // part of this FIFO queue.
            if !crate::providers::codex::has_pending_desktop_permission(app) {
                crate::window::maybe_close_after_permission(app);
            }
        }
    }
}

/// Whether the FIFO queue currently holds any request — checked by the Codex
/// desktop-permission mirror (`providers::codex::rollout`) before it decides
/// it's safe to auto-close the panel on its own resolution, since that path
/// is a separate map from this queue.
pub(crate) fn has_pending(app: &AppHandle) -> bool {
    app.try_state::<PendingQueue>()
        .map(|queue| {
            !queue
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .is_empty()
        })
        .unwrap_or(false)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tauri commands (frontend Approve/Deny buttons)
// ─────────────────────────────────────────────────────────────────────────────

/// `#[tauri::command]` Current head of the queue (if any), for the frontend
/// to hydrate from on init/reload instead of relying solely on the
/// `permission-pending` event — an event fired before the webview's
/// `listen()` subscription is attached, or a reload that happens while a
/// request is already queued, would otherwise leave the gate panel
/// unreachable even though the tray badge is correctly blinking (D42 review
/// fix). Read-only, no queue mutation.
#[tauri::command]
pub fn permission_pending_snapshot(app: AppHandle) -> Option<PendingPayload> {
    let queue = app.state::<PendingQueue>();
    let q = queue.lock().unwrap();
    q.front().map(|slot| {
        let provider_pending_count = q
            .iter()
            .filter(|pending| pending.request.provider == slot.request.provider)
            .count();
        PendingPayload::from_slot(slot, q.len(), provider_pending_count)
    })
}

#[tauri::command]
pub fn permission_approve(
    app: AppHandle,
    provider: crate::providers::ProviderId,
    id: String,
) -> Result<(), String> {
    resolve(&app, provider, &id, DecisionMsg::plain(Decision::Allow))
}

#[tauri::command]
pub fn permission_deny(
    app: AppHandle,
    provider: crate::providers::ProviderId,
    id: String,
) -> Result<(), String> {
    resolve(&app, provider, &id, DecisionMsg::plain(Decision::Deny))
}

/// `#[tauri::command]` Approve + remember: mirrors `permission_approve`, but
/// also persists a Bash rule to `<repo>/.claude/settings.local.json`, or
/// registers a session-scoped in-memory always-allow entry for Read/Edit/
/// Write — matching Claude Code's own "Yes, don't ask again" split exactly
/// (see `always_allow` module docs). Errors if the tool has no safe field to
/// build a rule from — the frontend already hides this affordance in that
/// case (`always_allow_available`), so this is a defensive check, not a
/// normal UX path.
#[tauri::command]
pub fn permission_approve_always(
    app: AppHandle,
    provider: crate::providers::ProviderId,
    id: String,
) -> Result<(), String> {
    let queue = app.state::<PendingQueue>();
    let result = {
        let mut q = queue.lock().unwrap();
        let idx = q
            .iter()
            .position(|slot| slot.request.provider == provider && slot.request.request_id == id)
            .ok_or("no such pending permission request (it may have timed out)")?;
        let request = &q[idx].request;
        if request.provider == crate::providers::ProviderId::Claude
            && request.permission_suggestions.len() > 1
        {
            return Err(
                "multiple native Always Allow choices require an explicit selection".into(),
            );
        }
        let native_suggestion = (request.provider == crate::providers::ProviderId::Claude)
            .then(|| request.permission_suggestions.first().cloned())
            .flatten();
        if let Some(suggestion) = native_suggestion {
            let slot = q
                .remove(idx)
                .expect("idx just found on the same locked queue");
            slot.reply_tx
                .send(DecisionMsg::allow_with_permissions(vec![suggestion]))
                .map_err(|_| "permission request disconnected before decision".to_string())
        } else {
            let session_id = request.session_id.clone();
            let tool_name = request.tool_name.clone();
            let cwd = request.cwd.clone();
            let matched = local_always_allow_match(request)
                .ok_or("always-allow isn't available for this tool")?;

            // Keep the queue locked while persistence completes and until the
            // decision is placed in the channel. A timeout worker may already
            // be waiting, but it cannot observe the slot as absent before the
            // response exists for its final try_recv recovery path.
            let persist_result =
                always_allow::remember(&app, provider, &session_id, &tool_name, &cwd, &matched);
            let slot = q
                .remove(idx)
                .expect("slot remains claimed under the same queue lock");
            finish_fallback_decision(slot.reply_tx, persist_result)
        }
    };
    emit_state(&app, &queue);
    result
}

fn finish_fallback_decision(
    reply_tx: Sender<DecisionMsg>,
    persist_result: Result<(), String>,
) -> Result<(), String> {
    match persist_result {
        Ok(()) => reply_tx
            .send(DecisionMsg::plain(Decision::Allow))
            .map_err(|_| "permission request disconnected before decision".to_string()),
        Err(error) => {
            // Dropping the claimed slot's sender closes the hook connection
            // without a decision. Claude then falls back to its native UI;
            // a failed local policy write must never approve the tool.
            drop(reply_tx);
            Err(error)
        }
    }
}

/// Looks up `id` by scanning the queue (not a blind front-pop) — a stale
/// click can race a timeout that already dropped the same entry, so a
/// missing id is reported as an error rather than silently resolving the
/// wrong (current-front) request.
fn resolve(
    app: &AppHandle,
    provider: crate::providers::ProviderId,
    id: &str,
    decision: DecisionMsg,
) -> Result<(), String> {
    let queue = app.state::<PendingQueue>();
    let slot = {
        let mut q = queue.lock().unwrap();
        remove_pending(&mut q, provider, id)
            .ok_or("no such pending permission request (it may have timed out)")?
    };
    let _ = slot.reply_tx.send(decision);
    emit_state(app, &queue);
    Ok(())
}

fn remove_pending(
    queue: &mut VecDeque<PendingSlot>,
    provider: crate::providers::ProviderId,
    id: &str,
) -> Option<PendingSlot> {
    let idx = queue
        .iter()
        .position(|slot| slot.request.provider == provider && slot.request.request_id == id)?;
    queue.remove(idx)
}

fn enqueue_pending(queue: &mut VecDeque<PendingSlot>, slot: PendingSlot) -> bool {
    if queue.iter().any(|pending| {
        pending.request.provider == slot.request.provider
            && pending.request.request_id == slot.request.request_id
    }) {
        return false;
    }
    queue.push_back(slot);
    true
}

// ─────────────────────────────────────────────────────────────────────────────
// Socket listener (dedicated thread, unix only)
// ─────────────────────────────────────────────────────────────────────────────

/// Registers the shared queue (and the always-allow session set) as Tauri
/// state (cross-platform — commands must be able to resolve
/// `app.state::<PendingQueue>()`/`app.state::<AlwaysAllowSet>()` even where
/// the listener itself doesn't run) and starts the socket-accept thread
/// where supported.
pub fn start(app: AppHandle) {
    let queue: PendingQueue = Arc::new(Mutex::new(VecDeque::new()));
    app.manage(queue.clone());
    app.manage::<PermissionEmissionLock>(Arc::new(Mutex::new(())));
    app.manage(always_allow::new_set());
    app.manage::<PermissionActivityState>(Arc::new(Mutex::new(std::collections::HashMap::new())));
    transport::spawn_listener(app, queue);
}

fn record_permission_activity(app: &AppHandle, request: &HookRequest) {
    let verified_hash = match request.provider {
        crate::providers::ProviderId::Claude => Some(None),
        crate::providers::ProviderId::Codex => app
            .try_state::<crate::providers::codex::app_server::AccountSensorState>()
            .and_then(|state| {
                state
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .permission_hook
                    .clone()
            })
            .filter(|probe| probe.enabled && probe.trust_status == "trusted")
            .and_then(|probe| probe.current_hash.map(Some)),
    };
    let verified = verified_hash.is_some();
    if let Some(hook_hash) = verified_hash {
        app.state::<PermissionActivityState>()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(
                request.provider,
                PermissionActivity {
                    observed_at_ms: crate::providers::now_epoch_ms(),
                    hook_hash,
                },
            );
    }
    crate::providers::emit_health(
        app,
        request.provider,
        crate::providers::ProviderComponent::Permissions,
        if verified {
            crate::providers::HealthStatus::Connected
        } else {
            crate::providers::HealthStatus::Degraded
        },
        Some(if verified {
            "permission hook exchange observed".into()
        } else {
            "permission hook exchange observed; trust inventory unavailable".into()
        }),
    );
}

fn is_always_allowed(app: &AppHandle, request: &HookRequest) -> bool {
    if !request.permission_suggestions.is_empty() {
        return false;
    }
    let Some(matched) = local_always_allow_match(request) else {
        return false;
    };
    always_allow::is_remembered(
        &app.state::<AlwaysAllowSet>(),
        request.provider,
        &request.session_id,
        &request.tool_name,
        &matched,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{self, Receiver};
    use std::thread;

    fn pending_slot(request_id: &str, prompt_id: &str) -> (PendingSlot, Receiver<DecisionMsg>) {
        let (reply_tx, reply_rx) = mpsc::channel();
        (
            PendingSlot {
                request: HookRequest {
                    provider: crate::providers::ProviderId::Claude,
                    request_id: request_id.into(),
                    prompt_id: Some(prompt_id.into()),
                    session_id: "session-1".into(),
                    turn_id: None,
                    model: None,
                    permission_mode: None,
                    tool_name: "Bash".into(),
                    tool_input: serde_json::json!({ "command": "npm test" }),
                    cwd: "/tmp".into(),
                    permission_suggestions: Vec::new(),
                },
                reply_tx,
                project: "project".into(),
                branch: None,
                expires_at: std::time::SystemTime::now(),
            },
            reply_rx,
        )
    }

    #[test]
    fn two_requests_from_same_prompt_resolve_independently() {
        let (first, first_rx) = pending_slot("request-1", "shared-prompt");
        let (second, second_rx) = pending_slot("request-2", "shared-prompt");
        let mut queue = VecDeque::from([first, second]);

        let second = remove_pending(
            &mut queue,
            crate::providers::ProviderId::Claude,
            "request-2",
        )
        .unwrap();
        second
            .reply_tx
            .send(DecisionMsg::plain(Decision::Deny))
            .unwrap();

        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].request.request_id, "request-1");
        assert_eq!(
            serde_json::to_value(PendingPayload::from_slot(&queue[0], 1, 1)).unwrap()["provider"],
            "claude"
        );
        assert!(first_rx.try_recv().is_err());
        assert!(matches!(
            second_rx.try_recv().unwrap().decision,
            Decision::Deny
        ));
    }

    #[test]
    fn duplicate_request_id_is_rejected_without_replacing_original() {
        let (first, _first_rx) = pending_slot("duplicate", "prompt-1");
        let (duplicate, duplicate_rx) = pending_slot("duplicate", "prompt-2");
        let mut queue = VecDeque::new();

        assert!(enqueue_pending(&mut queue, first));
        assert!(!enqueue_pending(&mut queue, duplicate));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].request.prompt_id.as_deref(), Some("prompt-1"));
        assert!(matches!(
            duplicate_rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn same_request_id_isolated_by_provider() {
        let (claude, _claude_rx) = pending_slot("shared", "prompt-1");
        let (mut codex, _codex_rx) = pending_slot("shared", "prompt-2");
        codex.request.provider = crate::providers::ProviderId::Codex;
        codex.request.prompt_id = None;
        codex.request.turn_id = Some("turn-1".into());
        let mut queue = VecDeque::new();

        assert!(enqueue_pending(&mut queue, claude));
        assert!(enqueue_pending(&mut queue, codex));
        assert!(
            remove_pending(&mut queue, crate::providers::ProviderId::Codex, "shared").is_some()
        );
        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue[0].request.provider,
            crate::providers::ProviderId::Claude
        );
    }

    #[test]
    fn codex_always_allow_key_uses_exact_tool_input() {
        let (mut slot, _rx) = pending_slot("request-1", "prompt-1");
        slot.request.provider = crate::providers::ProviderId::Codex;
        slot.request.tool_name = "mcp__github__create_issue".into();
        slot.request.tool_input = serde_json::json!({ "title": "One", "repo": "x/y" });
        assert_eq!(
            local_always_allow_match(&slot.request).as_deref(),
            Some("{\"repo\":\"x/y\",\"title\":\"One\"}")
        );
    }

    #[test]
    fn native_suggestion_disables_local_always_allow_fast_path() {
        let (mut slot, _rx) = pending_slot("request-1", "prompt-1");
        assert_eq!(
            local_always_allow_match(&slot.request).as_deref(),
            Some("npm test")
        );
        slot.request.permission_suggestions = vec![serde_json::json!({
            "type": "addRules",
            "rules": [{ "toolName": "Bash", "ruleContent": "npm test" }],
            "behavior": "allow",
            "destination": "localSettings"
        })];
        assert!(local_always_allow_match(&slot.request).is_none());
    }

    #[test]
    fn failed_fallback_persistence_sends_no_approval() {
        let (reply_tx, reply_rx) = mpsc::channel();
        let result = finish_fallback_decision(reply_tx, Err("write failed".into()));
        assert_eq!(result.unwrap_err(), "write failed");
        assert!(matches!(
            reply_rx.try_recv(),
            Err(mpsc::TryRecvError::Disconnected)
        ));
    }

    #[test]
    fn disconnected_fallback_channel_is_not_reported_as_success() {
        let (reply_tx, reply_rx) = mpsc::channel();
        drop(reply_rx);
        let error = finish_fallback_decision(reply_tx, Ok(())).unwrap_err();
        assert_eq!(error, "permission request disconnected before decision");
    }

    #[test]
    fn timeout_cleanup_cannot_observe_claim_absent_before_decision() {
        let (slot, reply_rx) = pending_slot("request-1", "prompt-1");
        let queue = Arc::new(Mutex::new(VecDeque::from([slot])));
        let queue_for_timeout = queue.clone();
        let (attempting_tx, attempting_rx) = mpsc::channel();

        let mut claimed = queue.lock().unwrap();
        let timeout = thread::spawn(move || {
            attempting_tx.send(()).unwrap();
            let queue = queue_for_timeout.lock().unwrap();
            assert!(queue.is_empty());
            reply_rx.try_recv().unwrap().decision
        });
        attempting_rx.recv().unwrap();

        let slot = claimed.pop_front().unwrap();
        slot.reply_tx
            .send(DecisionMsg::plain(Decision::Allow))
            .unwrap();
        drop(claimed);

        assert!(matches!(timeout.join().unwrap(), Decision::Allow));
    }

    #[test]
    fn cwd_project_takes_basename() {
        assert_eq!(cwd_project("/code/web-app"), "web-app");
        assert_eq!(cwd_project("/code/web-app/"), "web-app");
    }

    #[test]
    fn cwd_project_edge_cases() {
        assert_eq!(cwd_project(""), "?");
        assert_eq!(cwd_project("/"), "/");
    }

    #[test]
    fn permission_socket_prefers_private_temp_directory() {
        let path = socket_path_from(
            Some(std::ffi::OsString::from("/private/user-tmp")),
            Some(std::ffi::OsString::from("/Users/test")),
        );
        assert_eq!(
            path,
            Some(PathBuf::from(
                "/private/user-tmp/.cc-autobahn/permission.sock"
            ))
        );
    }

    #[test]
    fn permission_socket_falls_back_to_home_without_tmpdir() {
        let path = socket_path_from(None, Some(std::ffi::OsString::from("/Users/test")));
        assert_eq!(
            path,
            Some(PathBuf::from("/Users/test/.cc-autobahn/permission.sock"))
        );
    }

    #[test]
    fn socket_candidates_include_both_tmpdir_and_home_when_they_differ() {
        let candidates = socket_path_candidates_from(
            Some(std::ffi::OsString::from("/private/user-tmp")),
            Some(std::ffi::OsString::from("/Users/test")),
        );
        assert_eq!(
            candidates,
            vec![
                PathBuf::from("/private/user-tmp/.cc-autobahn/permission.sock"),
                PathBuf::from("/Users/test/.cc-autobahn/permission.sock"),
            ]
        );
    }

    #[test]
    fn socket_candidates_dedupe_when_tmpdir_is_unset() {
        let candidates =
            socket_path_candidates_from(None, Some(std::ffi::OsString::from("/Users/test")));
        assert_eq!(
            candidates,
            vec![PathBuf::from("/Users/test/.cc-autobahn/permission.sock")]
        );
    }

    #[test]
    fn summarize_tool_input_shows_first_question_only() {
        let input = serde_json::json!({
            "questions": [
                { "question": "Which library?", "header": "Lib", "options": [] }
            ]
        });
        assert_eq!(
            summarize_tool_input("AskUserQuestion", &input),
            "Which library?"
        );
    }

    #[test]
    fn summarize_tool_input_counts_extra_questions() {
        let input = serde_json::json!({
            "questions": [
                { "question": "Which library?", "header": "Lib", "options": [] },
                { "question": "Which approach?", "header": "Approach", "options": [] }
            ]
        });
        assert_eq!(
            summarize_tool_input("AskUserQuestion", &input),
            "Which library? (+1 more)"
        );
    }

    #[test]
    fn summarize_tool_input_falls_back_to_raw_json_for_truly_unmapped_shapes() {
        let input = serde_json::json!({ "arbitrary": "field" });
        assert_eq!(
            summarize_tool_input("mcp__weird__tool", &input),
            input.to_string()
        );
    }

    #[test]
    fn summarize_tool_input_still_matches_known_flat_fields() {
        let input = serde_json::json!({ "command": "npm run build" });
        assert_eq!(summarize_tool_input("Bash", &input), "npm run build");
    }
}
