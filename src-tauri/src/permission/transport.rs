//! Transport layer for the permission gate: the Unix-domain-socket listener
//! and the sandbox-compatible file bridge, plus the per-connection handlers
//! that queue a request and wait for a human decision.
//!
//! The queue mechanics, wire types, and Tauri commands live in the parent
//! module; this submodule owns only how a blocking hook process reaches the
//! GUI (socket vs file bridge) and the liveness/timeout dance around a single
//! request/response round trip. It reaches back into the parent for the shared
//! queue (`emit_state`/`enqueue_pending`), the always-allow fast path, and the
//! path/validation helpers.

use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};

use tauri::AppHandle;

use super::{
    cwd_project, emit_state, enqueue_pending, file_bridge_dirs, git_branch, is_always_allowed,
    record_permission_activity, safe_request_id, socket_path, Decision, DecisionMsg, HookRequest,
    PendingQueue, PendingSlot,
};

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
///
/// `pub(super)` so `hook_bin` can size its own read-timeout just above it.
pub(super) const QUEUE_TIMEOUT_SECS: u64 = 60;

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

/// Codex/OWL denies Unix-domain socket connects from its tool sandbox. The
/// file bridge below is the sandbox-compatible fallback; short polling keeps
/// approval latency imperceptible without busy-spinning either process.
/// `pub(super)` so `hook_bin`'s client side polls at the same cadence.
pub(super) const FILE_BRIDGE_POLL_MS: u64 = 100;

/// Generous headroom over any realistic `tool_input` (even a large file
/// write), but still a hard cap: without one, a stuck or wrong-protocol
/// connection sending bytes without a `\n` would grow this long-running
/// process's memory without bound. `pub(super)` so `hook_bin` derives its
/// own response cap from the same base.
pub(super) const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

#[cfg(unix)]
pub(super) fn spawn_listener(app: AppHandle, queue: PendingQueue) {
    thread::spawn(move || {
        let Some(path) = socket_path() else { return };
        let Some(parent) = path.parent().map(|p| p.to_path_buf()) else {
            return;
        };
        if std::fs::create_dir_all(&parent).is_err() || set_directory_private(&parent).is_err() {
            return;
        }
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

        spawn_file_bridge(app.clone(), queue.clone());

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
pub(super) fn spawn_listener(_app: AppHandle, _queue: PendingQueue) {}

#[cfg(unix)]
fn set_socket_private(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o600);
    std::fs::set_permissions(path, perms)
}

#[cfg(unix)]
pub(super) fn set_directory_private(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(path)?.permissions();
    perms.set_mode(0o700);
    std::fs::set_permissions(path, perms)
}

#[cfg(unix)]
fn spawn_file_bridge(app: AppHandle, queue: PendingQueue) {
    thread::spawn(move || {
        let Some((requests, responses)) = file_bridge_dirs() else {
            return;
        };
        for dir in [&requests, &responses] {
            if std::fs::create_dir_all(dir).is_err() || set_directory_private(dir).is_err() {
                return;
            }
        }

        loop {
            let Ok(entries) = std::fs::read_dir(&requests) else {
                thread::sleep(Duration::from_millis(FILE_BRIDGE_POLL_MS));
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|value| value.to_str()) != Some("json") {
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                let _ = std::fs::remove_file(&path);
                let Ok(request) = serde_json::from_slice::<HookRequest>(&bytes) else {
                    continue;
                };
                if !safe_request_id(&request.request_id) {
                    continue;
                }
                let response_path = responses.join(format!("{}.json", request.request_id));
                let queue = queue.clone();
                let app = app.clone();
                thread::spawn(move || handle_file_request(request, response_path, queue, app));
            }
            thread::sleep(Duration::from_millis(FILE_BRIDGE_POLL_MS));
        }
    });
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
    record_permission_activity(&app, &request);

    // Always-allow fast path: a request matching a prior Always-Allow in
    // this session never gets queued or shown — the whole point is that the
    // human isn't asked again. Belt-and-suspenders for Bash (whose primary
    // persistence is the on-disk settings.local.json rule, checked by
    // Claude Code itself before it would even invoke the hook again) as
    // well as the sole mechanism for Read/Edit/Write (session-only, never on
    // disk — see `always_allow` module docs).
    if is_always_allowed(&app, &request) {
        respond(stream, DecisionMsg::plain(Decision::Allow));
        return;
    }

    queue_request(stream, queue, app, request);
}

#[cfg(unix)]
fn handle_file_request(
    request: HookRequest,
    response_path: PathBuf,
    queue: PendingQueue,
    app: AppHandle,
) {
    record_permission_activity(&app, &request);
    if is_always_allowed(&app, &request) {
        let _ = write_file_response(&response_path, &DecisionMsg::plain(Decision::Allow));
        return;
    }

    let id = request.request_id.clone();
    let provider = request.provider;
    let project = cwd_project(&request.cwd);
    let branch = git_branch(&request.cwd);
    let (tx, rx) = mpsc::channel::<DecisionMsg>();
    let expires_at = std::time::SystemTime::now() + Duration::from_secs(QUEUE_TIMEOUT_SECS);
    {
        let mut pending = queue.lock().unwrap();
        if !enqueue_pending(
            &mut pending,
            PendingSlot {
                request,
                reply_tx: tx,
                project,
                branch,
                expires_at,
            },
        ) {
            return;
        }
    }
    emit_state(&app, &queue);

    match rx.recv_timeout(Duration::from_secs(QUEUE_TIMEOUT_SECS)) {
        Ok(decision) => {
            let _ = write_file_response(&response_path, &decision);
        }
        Err(_) => {
            let still_queued = {
                let mut pending = queue.lock().unwrap();
                let existed = pending
                    .iter()
                    .any(|slot| slot.request.provider == provider && slot.request.request_id == id);
                pending.retain(|slot| {
                    slot.request.provider != provider || slot.request.request_id != id
                });
                existed
            };
            if !still_queued {
                if let Ok(decision) = rx.try_recv() {
                    let _ = write_file_response(&response_path, &decision);
                    return;
                }
            }
            emit_state(&app, &queue);
        }
    }
}

#[cfg(unix)]
fn write_file_response(path: &std::path::Path, decision: &DecisionMsg) -> std::io::Result<()> {
    use std::os::unix::fs::OpenOptionsExt;

    let bytes = serde_json::to_vec(decision).map_err(std::io::Error::other)?;
    let temp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temp)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    std::fs::rename(&temp, path)
}

#[cfg(unix)]
fn queue_request(stream: UnixStream, queue: PendingQueue, app: AppHandle, request: HookRequest) {
    let id = request.request_id.clone();
    let provider = request.provider;
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
                let existed = q
                    .iter()
                    .any(|s| s.request.provider == provider && s.request.request_id == id);
                q.retain(|slot| slot.request.provider != provider || slot.request.request_id != id);
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
