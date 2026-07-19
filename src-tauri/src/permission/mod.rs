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
pub mod hook_bin;
pub mod install;

use always_allow::AlwaysAllowSet;

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// GUI-side safety net: how long a request sits in the queue before this
/// connection thread gives up on a human ever clicking, closes the socket,
/// and lets the hook fail open (D-review: was 550s, just under `hook_bin`'s
/// own 580s read-timeout — but Claude Code has no documented way to tell our
/// hook a permission decision was already made through some other path (its
/// own terminal prompt, a different client), so an orphaned hook process
/// left our queue entry stuck on-screen for up to ~9 minutes with no way to
/// tell it wasn't actually just slow). Short enough that a stale entry
/// clears itself out fast; still well over `watch_peer`'s ~2s dead-peer
/// detection for the common case where the hook process *does* get killed.
const QUEUE_TIMEOUT_SECS: u64 = 60;

/// A well-behaved hook sends its request line immediately after connecting
/// — bounds how long a stalled or wrong-protocol peer can pin a connection
/// thread waiting for one. Independent of `QUEUE_TIMEOUT_SECS`: this guards
/// the socket read itself, not the wait for a human decision.
const REQUEST_READ_TIMEOUT_SECS: u64 = 10;

/// Read-timeout granularity for [`watch_peer`]'s liveness probe — bounds how
/// long a dead/killed hook process can go undetected once queued.
const PEER_PROBE_TIMEOUT_SECS: u64 = 1;

/// Poll granularity of [`handle_connection`]'s own wait loop: checks both
/// the decision channel and the peer-gone channel this often instead of one
/// blocking `QUEUE_TIMEOUT_SECS`-long recv. Combined with
/// `PEER_PROBE_TIMEOUT_SECS`, bounds detection of a dead peer to ~2s worst
/// case instead of up to 550s.
const DECISION_POLL_SECS: u64 = 1;

/// Generous headroom over any realistic `tool_input` (even a large file
/// write), but still a hard cap: without one, a stuck or wrong-protocol
/// connection sending bytes without a `\n` would grow this long-running
/// process's memory without bound.
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

/// `~/.claude/cc-autobahn/permission.sock` — request/response bridge between
/// a blocking hook process and this GUI.
pub(crate) fn socket_path() -> Option<PathBuf> {
    Some(
        crate::sensor::claude_config_dir()?
            .join("cc-autobahn")
            .join("permission.sock"),
    )
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
    /// Unique identity generated by this individual hook process. Claude's
    /// `prompt_id` correlates work to a user prompt and is neither required
    /// nor unique per permission request.
    pub(crate) request_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) prompt_id: Option<String>,
    pub(crate) session_id: String,
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
    always_allow_available: bool,
    expires_at_ms: i64,
}

impl PendingPayload {
    fn from_slot(slot: &PendingSlot, pending_count: usize) -> Self {
        PendingPayload {
            provider: crate::providers::ProviderId::Claude,
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
            always_allow_available: slot.request.permission_suggestions.len() == 1
                || (slot.request.permission_suggestions.is_empty()
                    && matched_field(&slot.request.tool_name, &slot.request.tool_input).is_some()),
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
/// actually decide whether to approve.
fn summarize_tool_input(tool_name: &str, input: &serde_json::Value) -> String {
    matched_field(tool_name, input).unwrap_or_else(|| input.to_string())
}

fn local_always_allow_match(request: &HookRequest) -> Option<String> {
    request
        .permission_suggestions
        .is_empty()
        .then(|| matched_field(&request.tool_name, &request.tool_input))
        .flatten()
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
        q.front()
            .map(|slot| PendingPayload::from_slot(slot, q.len()))
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
                    provider: crate::providers::ProviderId::Claude,
                },
            );
        }
    }
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
    q.front()
        .map(|slot| PendingPayload::from_slot(slot, q.len()))
}

#[tauri::command]
pub fn permission_approve(app: AppHandle, id: String) -> Result<(), String> {
    resolve(&app, &id, DecisionMsg::plain(Decision::Allow))
}

#[tauri::command]
pub fn permission_deny(app: AppHandle, id: String) -> Result<(), String> {
    resolve(&app, &id, DecisionMsg::plain(Decision::Deny))
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
pub fn permission_approve_always(app: AppHandle, id: String) -> Result<(), String> {
    let queue = app.state::<PendingQueue>();
    let result = {
        let mut q = queue.lock().unwrap();
        let idx = q
            .iter()
            .position(|slot| slot.request.request_id == id)
            .ok_or("no such pending permission request (it may have timed out)")?;
        let request = &q[idx].request;
        if request.permission_suggestions.len() > 1 {
            return Err(
                "multiple native Always Allow choices require an explicit selection".into(),
            );
        }
        if let Some(suggestion) = request.permission_suggestions.first().cloned() {
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
            let matched = matched_field(&request.tool_name, &request.tool_input)
                .ok_or("always-allow isn't available for this tool")?;

            // Keep the queue locked while persistence completes and until the
            // decision is placed in the channel. A timeout worker may already
            // be waiting, but it cannot observe the slot as absent before the
            // response exists for its final try_recv recovery path.
            let persist_result =
                always_allow::remember(&app, &session_id, &tool_name, &cwd, &matched);
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
fn resolve(app: &AppHandle, id: &str, decision: DecisionMsg) -> Result<(), String> {
    let queue = app.state::<PendingQueue>();
    let slot = {
        let mut q = queue.lock().unwrap();
        remove_pending_by_id(&mut q, id)
            .ok_or("no such pending permission request (it may have timed out)")?
    };
    let _ = slot.reply_tx.send(decision);
    emit_state(app, &queue);
    Ok(())
}

fn remove_pending_by_id(queue: &mut VecDeque<PendingSlot>, id: &str) -> Option<PendingSlot> {
    let idx = queue
        .iter()
        .position(|slot| slot.request.request_id == id)?;
    queue.remove(idx)
}

fn enqueue_pending(queue: &mut VecDeque<PendingSlot>, slot: PendingSlot) -> bool {
    if queue
        .iter()
        .any(|pending| pending.request.request_id == slot.request.request_id)
    {
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
    spawn_listener(app, queue);
}

#[cfg(unix)]
fn spawn_listener(app: AppHandle, queue: PendingQueue) {
    thread::spawn(move || {
        let Some(path) = socket_path() else { return };
        let Some(parent) = path.parent().map(|p| p.to_path_buf()) else {
            return;
        };
        let _ = std::fs::create_dir_all(&parent);
        let listener = match bind_listener(&path) {
            Ok(Some(listener)) => listener,
            // Another live cc-autobahn owns the socket. Never unlink it: doing
            // so makes the existing listener unreachable and steals all new
            // permission requests without stopping the old process.
            Ok(None) | Err(_) => return,
        };
        if set_socket_private(&path).is_err() {
            drop(listener);
            let _ = std::fs::remove_file(&path);
            return; // fail closed locally rather than expose an injectable socket
        }

        for conn in listener.incoming() {
            match conn {
                Ok(stream) => {
                    let app = app.clone();
                    let queue = queue.clone();
                    thread::spawn(move || handle_connection(stream, queue, app));
                }
                // A persistently failing accept() (e.g. fd exhaustion) would
                // otherwise spin this loop at 100% CPU instead of backing
                // off while whatever caused it is still true.
                Err(_) => thread::sleep(Duration::from_millis(200)),
            }
        }
    });
}

/// Binds a fresh socket, replaces only a provably stale socket, and leaves a
/// live listener untouched. `Ok(None)` means another process answered the
/// liveness probe and remains the owner.
#[cfg(unix)]
fn bind_listener(path: &std::path::Path) -> std::io::Result<Option<UnixListener>> {
    use std::os::unix::fs::FileTypeExt;

    match UnixListener::bind(path) {
        Ok(listener) => Ok(Some(listener)),
        Err(bind_error) if bind_error.kind() == std::io::ErrorKind::AddrInUse => {
            if UnixStream::connect(path).is_ok() {
                return Ok(None);
            }
            // Do not delete an unexpected regular file/symlink at our known
            // path. Only a dead Unix socket is eligible for stale cleanup.
            if !std::fs::symlink_metadata(path)?.file_type().is_socket() {
                return Err(bind_error);
            }
            std::fs::remove_file(path)?;
            UnixListener::bind(path).map(Some)
        }
        Err(error) => Err(error),
    }
}

#[cfg(not(unix))]
fn spawn_listener(_app: AppHandle, _queue: PendingQueue) {}

#[cfg(unix)]
fn set_socket_private(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)
}

/// One connection = one hook invocation = one permission request. Reads the
/// request line, queues it, waits for a decision (`permission_approve`/
/// `permission_deny`) OR the peer connection dying (`watch_peer`), and
/// either writes the response line back or — on timeout/peer-gone — drops
/// its own entry and lets the connection close silently (the hook's own,
/// shorter, read-timeout is what actually triggers fail-open on its side).
#[cfg(unix)]
fn handle_connection(stream: UnixStream, queue: PendingQueue, app: AppHandle) {
    // Bounds the initial request read (both time and size — see the const
    // docs above); does NOT affect the later response write's own deadline,
    // since SO_RCVTIMEO/SO_SNDTIMEO only bound how long a single syscall may
    // block, not a wall-clock deadline from when they were set.
    let _ = stream.set_read_timeout(Some(Duration::from_secs(REQUEST_READ_TIMEOUT_SECS)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(REQUEST_READ_TIMEOUT_SECS)));

    let cloned = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let mut reader = BufReader::new(cloned).take(MAX_REQUEST_BYTES);
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let Ok(request) = serde_json::from_str::<HookRequest>(&line) else {
        return;
    };

    // Always-allow fast path: a request matching a prior Always-Allow in
    // this session never gets queued or shown — the whole point is that the
    // human isn't asked again. Belt-and-suspenders for Bash (whose primary
    // persistence is the on-disk settings.local.json rule, checked by
    // Claude Code itself before it would even invoke the hook again) as
    // well as the sole mechanism for Read/Edit/Write (session-only, never on
    // disk — see `always_allow` module docs).
    if request.permission_suggestions.is_empty() {
        let Some(matched) = local_always_allow_match(&request) else {
            return queue_request(stream, queue, app, request);
        };
        let always_allow = app.state::<AlwaysAllowSet>();
        if always_allow::is_remembered(
            &always_allow,
            &request.session_id,
            &request.tool_name,
            &matched,
        ) {
            respond(stream, DecisionMsg::plain(Decision::Allow));
            return;
        }
    }

    queue_request(stream, queue, app, request);
}

#[cfg(unix)]
fn queue_request(stream: UnixStream, queue: PendingQueue, app: AppHandle, request: HookRequest) {
    let id = request.request_id.clone();
    let project = cwd_project(&request.cwd);
    let branch = git_branch(&request.cwd);
    let (tx, rx) = mpsc::channel::<DecisionMsg>();
    let expires_at = std::time::SystemTime::now() + Duration::from_secs(QUEUE_TIMEOUT_SECS);
    {
        let mut q = queue.lock().unwrap();
        let inserted = enqueue_pending(
            &mut q,
            PendingSlot {
                request,
                reply_tx: tx,
                project,
                branch,
                expires_at,
            },
        );
        if !inserted {
            return;
        }
    }
    emit_state(&app, &queue);

    // Peer-liveness detection: a companion thread blocks on the same
    // connection watching for it to close (hook process killed, or Claude
    // Code itself gave up on a hung hook and resolved the request some
    // other way) so a dead peer clears its queue entry in ~2s instead of
    // waiting out the full QUEUE_TIMEOUT_SECS. `None` (no watcher) if
    // `try_clone` fails — e.g. fd exhaustion — degrading gracefully to the
    // old timeout-only behavior rather than failing the request.
    let (peer_gone_tx, peer_gone_rx) = mpsc::channel::<()>();
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = stream.try_clone().ok().map(|watch_stream| {
        let stop = stop.clone();
        thread::spawn(move || watch_peer(watch_stream, peer_gone_tx, stop))
    });

    let outcome = wait_outcome(&rx, &peer_gone_rx, Duration::from_secs(QUEUE_TIMEOUT_SECS));
    // Signal the watcher to stop as soon as we have an outcome — joined
    // below, after respond(), so this never delays the actual reply.
    stop.store(true, Ordering::Relaxed);

    match outcome {
        WaitOutcome::Decided(decision) => respond(stream, decision),
        WaitOutcome::TimedOut | WaitOutcome::PeerGone => {
            let still_queued = {
                let mut q = queue.lock().unwrap();
                let existed = q.iter().any(|s| s.request.request_id == id);
                q.retain(|slot| slot.request.request_id != id);
                existed
            };
            // `recovered` mirrors the pre-peer-liveness behavior exactly:
            // `emit_state` is skipped only when `resolve()` already ran it
            // for us (i.e. a click/auto-resolve raced the timeout/peer-gone
            // signal and won).
            let mut recovered = false;
            if !still_queued {
                // `permission_approve`/`permission_deny` already removed our
                // entry between the outcome maturing and this lock — a click
                // (or an auto-resolve) landed at the exact instant the
                // timeout/peer-gone signal fired. The decision may already
                // be sitting in the channel (send() doesn't fail just
                // because the wait loop gave up first); pick it up instead
                // of silently discarding a real human decision while the
                // frontend believes it succeeded.
                if let Ok(decision) = rx.try_recv() {
                    respond(stream, decision);
                    recovered = true;
                }
            }
            if !recovered {
                emit_state(&app, &queue);
            }
        }
    }
    if let Some(w) = watcher {
        let _ = w.join();
    }
}

enum WaitOutcome {
    Decided(DecisionMsg),
    PeerGone,
    TimedOut,
}

/// Polls both `rx` (a decision from `permission_approve`/`permission_deny`)
/// and `peer_gone_rx` (fired by [`watch_peer`] when the hook's connection
/// closes) at `DECISION_POLL_SECS` granularity instead of a single long
/// blocking wait, so a dead peer is noticed quickly. PURE (given
/// already-open channels) → testable without real sockets.
fn wait_outcome(
    rx: &Receiver<DecisionMsg>,
    peer_gone_rx: &Receiver<()>,
    total: Duration,
) -> WaitOutcome {
    let start = Instant::now();
    loop {
        let elapsed = start.elapsed();
        if elapsed >= total {
            return WaitOutcome::TimedOut;
        }
        let poll = Duration::from_secs(DECISION_POLL_SECS).min(total - elapsed);
        match rx.recv_timeout(poll) {
            Ok(decision) => return WaitOutcome::Decided(decision),
            Err(RecvTimeoutError::Disconnected) => return WaitOutcome::TimedOut,
            Err(RecvTimeoutError::Timeout) => {
                if peer_gone_rx.try_recv().is_ok() {
                    // A decision may have landed in the exact instant the
                    // peer-gone signal fired — same race as the plain
                    // timeout path, give `rx` one last chance before
                    // declaring the peer gone.
                    return match rx.try_recv() {
                        Ok(decision) => WaitOutcome::Decided(decision),
                        Err(_) => WaitOutcome::PeerGone,
                    };
                }
            }
        }
    }
}

/// Spawned per connection right after queuing. Blocks on small reads of a
/// cloned handle to the same socket — the hook process never sends a second
/// line per protocol (see [`HookRequest`]/[`DecisionMsg`]), so any read
/// outcome here is a liveness signal, not real data: `Ok(0)` (clean EOF) or
/// a reset/broken-pipe `Err` means the peer is gone; a timeout
/// (WouldBlock/TimedOut, from `PEER_PROBE_TIMEOUT_SECS`) means "still alive,
/// keep waiting"; `Ok(n>0)` (a protocol violation) is also treated as "still
/// alive" since only a connected peer can send bytes. Exits as soon as
/// `stop` is set (checked every wake — `handle_connection` does this once it
/// has its own outcome), or after `QUEUE_TIMEOUT_SECS` regardless, as a
/// defensive cap against ever outliving `handle_connection`'s own guaranteed
/// exit paths.
#[cfg(unix)]
fn watch_peer(mut stream: UnixStream, gone_tx: Sender<()>, stop: Arc<AtomicBool>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(PEER_PROBE_TIMEOUT_SECS)));
    let deadline = Instant::now() + Duration::from_secs(QUEUE_TIMEOUT_SECS);
    let mut buf = [0u8; 1];
    loop {
        if stop.load(Ordering::Relaxed) || Instant::now() >= deadline {
            return;
        }
        match stream.read(&mut buf) {
            Ok(0) => {
                let _ = gone_tx.send(());
                return;
            }
            Ok(_) => continue, // unexpected bytes, but proves the peer is alive
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue
            }
            Err(_) => {
                let _ = gone_tx.send(());
                return;
            }
        }
    }
}

#[cfg(unix)]
fn respond(stream: UnixStream, response: DecisionMsg) {
    if let Ok(json) = serde_json::to_string(&response) {
        let mut stream = stream;
        let _ = writeln!(stream, "{json}");
        let _ = stream.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: Duration = Duration::from_millis(50);

    #[test]
    fn decided_immediately() {
        let (tx, rx) = mpsc::channel::<DecisionMsg>();
        let (_gone_tx, gone_rx) = mpsc::channel::<()>();
        tx.send(DecisionMsg::plain(Decision::Allow)).unwrap();
        match wait_outcome(&rx, &gone_rx, SHORT) {
            WaitOutcome::Decided(response) if matches!(response.decision, Decision::Allow) => {}
            _ => panic!("expected Decided(Allow)"),
        }
    }

    #[test]
    fn times_out_when_nothing_arrives() {
        let (_tx, rx) = mpsc::channel::<DecisionMsg>();
        let (_gone_tx, gone_rx) = mpsc::channel::<()>();
        assert!(matches!(
            wait_outcome(&rx, &gone_rx, SHORT),
            WaitOutcome::TimedOut
        ));
    }

    #[test]
    fn peer_gone_wins_when_no_decision_follows() {
        let (_tx, rx) = mpsc::channel::<DecisionMsg>();
        let (gone_tx, gone_rx) = mpsc::channel::<()>();
        gone_tx.send(()).unwrap();
        assert!(matches!(
            wait_outcome(&rx, &gone_rx, SHORT),
            WaitOutcome::PeerGone
        ));
    }

    /// A decision that lands in the exact instant the peer-gone signal fires
    /// must still win — mirrors the pre-existing timeout/approve race
    /// handling, now extended to cover the peer-liveness path too.
    #[test]
    fn decision_wins_the_race_with_peer_gone() {
        let (tx, rx) = mpsc::channel::<DecisionMsg>();
        let (gone_tx, gone_rx) = mpsc::channel::<()>();
        gone_tx.send(()).unwrap();
        tx.send(DecisionMsg::plain(Decision::Deny)).unwrap();
        match wait_outcome(&rx, &gone_rx, SHORT) {
            WaitOutcome::Decided(response) if matches!(response.decision, Decision::Deny) => {}
            _ => panic!("expected Decided(Deny), got a different outcome"),
        }
    }

    fn pending_slot(request_id: &str, prompt_id: &str) -> (PendingSlot, Receiver<DecisionMsg>) {
        let (reply_tx, reply_rx) = mpsc::channel();
        (
            PendingSlot {
                request: HookRequest {
                    request_id: request_id.into(),
                    prompt_id: Some(prompt_id.into()),
                    session_id: "session-1".into(),
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

        let second = remove_pending_by_id(&mut queue, "request-2").unwrap();
        second
            .reply_tx
            .send(DecisionMsg::plain(Decision::Deny))
            .unwrap();

        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].request.request_id, "request-1");
        assert_eq!(
            serde_json::to_value(PendingPayload::from_slot(&queue[0], 1)).unwrap()["provider"],
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

    #[cfg(unix)]
    fn test_socket_path(case: &str) -> PathBuf {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        PathBuf::from("/tmp").join(format!("cca-{case}-{}-{unique}.sock", std::process::id()))
    }

    #[cfg(unix)]
    #[test]
    fn bind_listener_keeps_live_owner() {
        let path = test_socket_path("live");
        let first = bind_listener(&path).unwrap().unwrap();
        assert!(
            bind_listener(&path).unwrap().is_none(),
            "second instance must not unlink a live listener"
        );
        drop(first);
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn bind_listener_replaces_stale_socket() {
        let path = test_socket_path("stale");
        let stale = UnixListener::bind(&path).unwrap();
        drop(stale); // leaves a socket inode with no live listener

        let replacement = bind_listener(&path).unwrap();
        assert!(replacement.is_some());
        drop(replacement);
        let _ = std::fs::remove_file(path);
    }

    #[cfg(unix)]
    #[test]
    fn bind_listener_never_deletes_regular_file() {
        let path = test_socket_path("regular");
        std::fs::write(&path, b"keep me").unwrap();

        assert!(bind_listener(&path).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"keep me");
        let _ = std::fs::remove_file(path);
    }
}
