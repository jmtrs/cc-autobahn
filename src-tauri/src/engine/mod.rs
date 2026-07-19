//! engine — data engine: ccusage detection + `blocks` polling.
//!
//! All I/O lives here (never in the UI). We don't fork ccusage: we run it
//! as a child process and parse its `--json` output (see docs/ARCHITECTURE.md, D1–D3).
//!
//! Deliberately sober design (no plugins, no async framework): a dedicated
//! thread with `std::process::Command` + `std::thread::sleep`. Robust,
//! serviceable, no dependencies beyond serde. The loop never panics;
//! every failure is turned into an event towards the frontend.

mod blocks;
pub mod history;
pub mod install;

use std::process::Command;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

/// Cadence for `ccusage blocks` (D13: 10–30 s). The 5 h block changes slowly;
/// polling every second would be a wasteful process spawn.
const POLL_INTERVAL_SECS: u64 = 15;
/// Cap for the backoff below — never wait longer than this between retries.
const MAX_BACKOFF_SECS: u64 = 120;
/// Consecutive failures before re-running `detect()` (the resolved engine may
/// have been uninstalled/moved; falls back through the global → npx → bunx cascade).
const REDETECT_AFTER_FAILURES: u32 = 4;

// ─────────────────────────────────────────────────────────────────────────────
// Engine detection (cascade D9: global → npx → bunx → none)
// ─────────────────────────────────────────────────────────────────────────────

/// How ccusage is invoked, resolved once at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Engine {
    /// `ccusage` on the PATH (global install).
    Global,
    /// `npx -y ccusage@latest` (Node present, nothing installed).
    Npx,
    /// `bunx ccusage` (Bun present).
    Bunx,
}

impl Engine {
    /// Base command; the caller appends `blocks --active --json`. `path`, when
    /// present, comes from `PathState` (the resolved-at-startup/post-install
    /// PATH — see `path_state.rs`) and is applied explicitly instead of
    /// relying on the inherited process environment.
    fn base_command(self, path: Option<&str>) -> Command {
        let mut c = match self {
            Engine::Global => Command::new("ccusage"),
            Engine::Npx => {
                let mut c = Command::new("npx");
                c.args(["-y", "ccusage@latest"]);
                c
            }
            Engine::Bunx => {
                let mut c = Command::new("bunx");
                c.arg("ccusage");
                c
            }
        };
        if let Some(p) = path {
            c.env("PATH", p);
        }
        c
    }

    /// Short label for the `app-engine-detected` event.
    fn label(self) -> &'static str {
        match self {
            Engine::Global => "ccusage",
            Engine::Npx => "npx",
            Engine::Bunx => "bunx",
        }
    }
}

/// Checks the PATH once and returns the first available engine (D9). `path`
/// comes from `PathState`; `None` falls back to `env_lock::var_os("PATH")`
/// (state not yet populated, e.g. called before `pathfix::apply`).
pub(crate) fn detect(path: Option<&str>) -> Option<Engine> {
    if on_path("ccusage", path) {
        Some(Engine::Global)
    } else if on_path("npx", path) {
        Some(Engine::Npx)
    } else if on_path("bunx", path) {
        Some(Engine::Bunx)
    } else {
        None
    }
}

/// `true` if `bin` resolves on the PATH. Walks `$PATH` by hand — no extra crate,
/// portable. On Windows it accounts for the usual executable extensions.
fn on_path(bin: &str, path: Option<&str>) -> bool {
    let owned;
    let path: &str = match path {
        Some(p) => p,
        None => {
            let Some(p) = crate::env_lock::var_os("PATH") else {
                return false;
            };
            owned = p;
            let Some(s) = owned.to_str() else {
                return false;
            };
            s
        }
    };
    let exts: &[&str] = if cfg!(windows) {
        &["", ".cmd", ".exe", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(path) {
        for ext in exts {
            let candidate = dir.join(format!("{bin}{ext}"));
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}

/// `#[tauri::command]` Is an engine available RIGHT NOW? Used to render the
/// "CHECK ENGINE" screen on the first render without depending on winning the
/// race against the `app-engine-missing` event (the `start` thread may emit it
/// before the frontend finishes registering the listener). Same pattern as `sensor_status`.
#[tauri::command]
pub fn engine_status(app: tauri::AppHandle) -> bool {
    let path = crate::path_state::get(&app.state::<crate::path_state::PathState>());
    detect(path.as_deref()).is_some()
}

/// Starts the engine on a dedicated thread. Detects once; if there's no engine
/// it emits `app-engine-missing` and returns. If there is one, polls in a loop emitting:
///   · `blocks-update`  → active block (payload = Block)
///   · `blocks-idle`    → no active block right now
///   · `app-engine-error`   → one-off failure of this cycle (payload = message)
///   · `app-engine-detected`→ once, with the engine's label
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        // Read PathState ONCE at thread entry (PATH only ever changes at
        // pathfix::apply on startup or install::prepend_path after a Bun
        // install — and that second case already re-spawns a fresh `start`
        // thread via install_bun, so re-reading per-iteration here would add
        // nothing but the risk of two engine threads racing).
        let path = crate::path_state::get(&app.state::<crate::path_state::PathState>());

        let mut engine = match detect(path.as_deref()) {
            Some(e) => e,
            None => {
                crate::providers::emit_health(
                    &app,
                    crate::providers::ProviderId::Claude,
                    crate::providers::ProviderComponent::Engine,
                    crate::providers::HealthStatus::Unavailable,
                    Some("ccusage engine not found".into()),
                );
                let _ = app.emit("app-engine-missing", ());
                return;
            }
        };
        let _ = app.emit("app-engine-detected", engine.label());
        crate::providers::emit_health(
            &app,
            crate::providers::ProviderId::Claude,
            crate::providers::ProviderComponent::Engine,
            crate::providers::HealthStatus::Connected,
            None,
        );

        let mut consecutive_failures: u32 = 0;
        let mut engine_degraded = false;

        loop {
            match blocks::poll_once(engine, path.as_deref()) {
                Ok(Some(mut block)) => {
                    consecutive_failures = 0;
                    block.observed_at_ms = crate::providers::now_epoch_ms();
                    if take_recovered(&mut engine_degraded) {
                        crate::providers::emit_health(
                            &app,
                            crate::providers::ProviderId::Claude,
                            crate::providers::ProviderComponent::Engine,
                            crate::providers::HealthStatus::Connected,
                            None,
                        );
                    }
                    // % remaining of the 5h window for the tray ring — same
                    // criterion as applyEstimated() in main.js. Reported
                    // unconditionally: `tray_icon::set_progress` itself
                    // arbitrates against the sensor's OFFICIAL writes (D39),
                    // so `engine` doesn't need to know whether it's active.
                    let pct_remaining = block
                        .projection
                        .as_ref()
                        .map(|p| p.remaining_minutes as f64 / crate::tray_icon::WINDOW_MIN * 100.0)
                        .unwrap_or(0.0);
                    crate::tray_icon::set_progress(
                        &app,
                        pct_remaining,
                        crate::tray_icon::ProgressSource::Estimated,
                    );
                    let _ = app.emit("blocks-update", &block);
                }
                Ok(None) => {
                    consecutive_failures = 0;
                    if take_recovered(&mut engine_degraded) {
                        crate::providers::emit_health(
                            &app,
                            crate::providers::ProviderId::Claude,
                            crate::providers::ProviderComponent::Engine,
                            crate::providers::HealthStatus::Connected,
                            None,
                        );
                    }
                    // No active block: window not being spent, ring full.
                    crate::tray_icon::set_progress(
                        &app,
                        100.0,
                        crate::tray_icon::ProgressSource::Estimated,
                    );
                    let _ = app.emit(
                        "blocks-idle",
                        crate::providers::ProviderMarker {
                            provider: crate::providers::ProviderId::Claude,
                        },
                    );
                }
                Err(message) => {
                    consecutive_failures += 1;
                    engine_degraded = true;
                    let _ = app.emit("app-engine-error", &message);
                    crate::providers::emit_health(
                        &app,
                        crate::providers::ProviderId::Claude,
                        crate::providers::ProviderComponent::Engine,
                        crate::providers::HealthStatus::Degraded,
                        Some(message),
                    );

                    // The resolved engine may have disappeared (uninstalled/moved);
                    // re-run the detection cascade instead of hammering a dead binary forever.
                    if consecutive_failures.is_multiple_of(REDETECT_AFTER_FAILURES) {
                        if let Some(e) = detect(path.as_deref()) {
                            if e != engine {
                                engine = e;
                                let _ = app.emit("app-engine-detected", engine.label());
                                crate::providers::emit_health(
                                    &app,
                                    crate::providers::ProviderId::Claude,
                                    crate::providers::ProviderComponent::Engine,
                                    crate::providers::HealthStatus::Connected,
                                    None,
                                );
                                engine_degraded = false;
                            }
                        } else {
                            crate::providers::emit_health(
                                &app,
                                crate::providers::ProviderId::Claude,
                                crate::providers::ProviderComponent::Engine,
                                crate::providers::HealthStatus::Unavailable,
                                Some("ccusage engine disappeared".into()),
                            );
                            let _ = app.emit("app-engine-missing", ());
                            return;
                        }
                    }
                }
            }

            // Exponential backoff on repeated failures, capped, so a dead
            // subprocess isn't respawned every POLL_INTERVAL_SECS forever.
            let backoff_secs = if consecutive_failures == 0 {
                POLL_INTERVAL_SECS
            } else {
                POLL_INTERVAL_SECS
                    .saturating_mul(1u64 << consecutive_failures.min(8))
                    .min(MAX_BACKOFF_SECS)
            };
            thread::sleep(Duration::from_secs(backoff_secs));
        }
    });
}

fn take_recovered(engine_degraded: &mut bool) -> bool {
    std::mem::replace(engine_degraded, false)
}

#[cfg(test)]
mod provider_health_tests {
    use super::take_recovered;

    #[test]
    fn successful_poll_recovers_degraded_engine_once() {
        let mut degraded = true;
        assert!(take_recovered(&mut degraded));
        assert!(!degraded);
        assert!(!take_recovered(&mut degraded));
    }
}
