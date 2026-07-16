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
pub mod install;

use std::process::Command;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter};

/// Cadence for `ccusage blocks` (D13: 10–30 s). The 5 h block changes slowly;
/// polling every second would be a wasteful process spawn.
const POLL_INTERVAL_SECS: u64 = 15;

// ─────────────────────────────────────────────────────────────────────────────
// Engine detection (cascade D9: global → npx → bunx → none)
// ─────────────────────────────────────────────────────────────────────────────

/// How ccusage is invoked, resolved once at startup.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Engine {
    /// `ccusage` on the PATH (global install).
    Global,
    /// `npx -y ccusage@latest` (Node present, nothing installed).
    Npx,
    /// `bunx ccusage` (Bun present).
    Bunx,
}

impl Engine {
    /// Base command; the caller appends `blocks --active --json`.
    fn base_command(self) -> Command {
        match self {
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
        }
    }

    /// Short label for the `engine-detected` event.
    fn label(self) -> &'static str {
        match self {
            Engine::Global => "ccusage",
            Engine::Npx => "npx",
            Engine::Bunx => "bunx",
        }
    }
}

/// Checks the PATH once and returns the first available engine (D9).
pub(crate) fn detect() -> Option<Engine> {
    if on_path("ccusage") {
        Some(Engine::Global)
    } else if on_path("npx") {
        Some(Engine::Npx)
    } else if on_path("bunx") {
        Some(Engine::Bunx)
    } else {
        None
    }
}

/// `true` if `bin` resolves on the PATH. Walks `$PATH` by hand — no extra crate,
/// portable. On Windows it accounts for the usual executable extensions.
fn on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let exts: &[&str] = if cfg!(windows) {
        &["", ".cmd", ".exe", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path) {
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
/// race against the `engine-missing` event (the `start` thread may emit it
/// before the frontend finishes registering the listener). Same pattern as `sensor_status`.
#[tauri::command]
pub fn engine_status() -> bool {
    detect().is_some()
}

/// Starts the engine on a dedicated thread. Detects once; if there's no engine
/// it emits `engine-missing` and returns. If there is one, polls in a loop emitting:
///   · `blocks-update`  → active block (payload = Block)
///   · `blocks-idle`    → no active block right now
///   · `engine-error`   → one-off failure of this cycle (payload = message)
///   · `engine-detected`→ once, with the engine's label
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        let engine = match detect() {
            Some(e) => e,
            None => {
                let _ = app.emit("engine-missing", ());
                return;
            }
        };
        let _ = app.emit("engine-detected", engine.label());

        loop {
            match blocks::poll_once(engine) {
                Ok(Some(block)) => {
                    // % remaining of the 5h window for the tray ring —
                    // same criterion as applyEstimated() in main.js.
                    let pct_remaining = block
                        .projection
                        .as_ref()
                        .map(|p| p.remaining_minutes as f64 / crate::tray_icon::WINDOW_MIN * 100.0)
                        .unwrap_or(0.0);
                    crate::tray_icon::set_progress(&app, pct_remaining);
                    let _ = app.emit("blocks-update", &block);
                }
                Ok(None) => {
                    // No active block: window not being spent, ring full.
                    crate::tray_icon::set_progress(&app, 100.0);
                    let _ = app.emit("blocks-idle", ());
                }
                Err(message) => {
                    let _ = app.emit("engine-error", message);
                }
            }
            thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
        }
    });
}
