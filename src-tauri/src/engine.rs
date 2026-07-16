//! engine — data engine: ccusage detection + `blocks` polling.
//!
//! All I/O lives here (never in the UI). We don't fork ccusage: we run it
//! as a child process and parse its `--json` output (see docs/ARCHITECTURE.md, D1–D3).
//!
//! Deliberately sober design (no plugins, no async framework): a dedicated
//! thread with `std::process::Command` + `std::thread::sleep`. Robust,
//! serviceable, no dependencies beyond serde. The loop never panics;
//! every failure is turned into an event towards the frontend.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Cadence for `ccusage blocks` (D13: 10–30 s). The 5 h block changes slowly;
/// polling every second would be a wasteful process spawn.
const POLL_INTERVAL_SECS: u64 = 15;

// ─────────────────────────────────────────────────────────────────────────────
// Engine detection (cascade D9: global → npx → bunx → none)
// ─────────────────────────────────────────────────────────────────────────────

/// How ccusage is invoked, resolved once at startup.
#[derive(Debug, Clone, Copy)]
enum Engine {
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
fn detect() -> Option<Engine> {
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

/// `#[tauri::command]` "INSTALL ENGINE" button (D9, Phase 4): runs Bun's
/// official installer, updates the `PATH` of the already-running process (the
/// installer only appends it to the shell rc, which this process never
/// re-reads) and retries the engine. `Err` with a readable message to render in the overlay.
#[tauri::command]
pub fn install_bun(app: AppHandle) -> Result<String, String> {
    if let Some(engine) = detect() {
        start(app);
        return Ok(engine.label().to_string());
    }

    run_bun_installer()?;

    if let Some(dir) = bun_bin_dir() {
        prepend_path(&dir);
    }

    match detect() {
        Some(engine) => {
            start(app);
            Ok(engine.label().to_string())
        }
        None => Err("Bun was installed but bunx isn't on PATH".to_string()),
    }
}

/// `~/.bun/bin`, the fixed destination of the official installer.
#[cfg(unix)]
fn bun_bin_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".bun").join("bin"))
}

#[cfg(not(unix))]
fn bun_bin_dir() -> Option<PathBuf> {
    None
}

/// Prepends `dir` to the current process's `PATH` (not the shell's) so that
/// `on_path` and subsequent `Command`s find `bunx` without restarting the app.
fn prepend_path(dir: &Path) {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(&existing));
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
    }
}

/// Official Bun installer (https://bun.sh/install). macOS/Linux only for
/// now — the rest of the project isn't tested on Windows either (D24).
#[cfg(unix)]
fn run_bun_installer() -> Result<(), String> {
    let status = Command::new("sh")
        .arg("-c")
        .arg("curl -fsSL https://bun.sh/install | bash")
        .status()
        .map_err(|e| format!("could not launch the Bun installer: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the Bun installer exited with {status}"))
    }
}

#[cfg(not(unix))]
fn run_bun_installer() -> Result<(), String> {
    Err("Automatic installation is only available on macOS/Linux for now. Install Bun manually from https://bun.sh and restart cc-autobahn.".to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// serde model for the `ccusage blocks --active --json` JSON
// Structured against the real output (ccusage v20; captured 2026-07-16).
// Optional/`default` fields because "gap" blocks omit several of them.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BlocksEnvelope {
    #[serde(default)]
    blocks: Vec<Block>,
}

/// A 5 h billing block. Forwarded as-is to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Block {
    id: String,
    #[serde(default)]
    is_active: bool,
    #[serde(default)]
    is_gap: bool,
    #[serde(default)]
    start_time: String,
    #[serde(default)]
    end_time: String,
    #[serde(default)]
    actual_end_time: Option<String>,
    #[serde(default)]
    cost_usd: f64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    entries: u64,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    token_counts: TokenCounts,
    #[serde(default)]
    burn_rate: Option<BurnRate>,
    #[serde(default)]
    projection: Option<Projection>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenCounts {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BurnRate {
    #[serde(default)]
    cost_per_hour: f64,
    #[serde(default)]
    tokens_per_minute: f64,
    /// Smoothed average from ccusage. NOT our per-response `tok/s` (D8):
    /// that one is computed by the JSONL tail in Phase 2.
    #[serde(default)]
    tokens_per_minute_for_indicator: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Projection {
    #[serde(default)]
    remaining_minutes: u64,
    #[serde(default)]
    total_cost: f64,
    #[serde(default)]
    total_tokens: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Polling
// ─────────────────────────────────────────────────────────────────────────────

/// Runs ccusage once and returns the active block (if any).
/// `Err` with a readable message on any spawn / exit / parse failure.
fn poll_once(engine: Engine) -> Result<Option<Block>, String> {
    let output = engine
        .base_command()
        .args(["blocks", "--active", "--json"])
        .output()
        .map_err(|e| format!("could not launch {}: {e}", engine.label()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ccusage exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    let envelope: BlocksEnvelope = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("unparseable ccusage JSON: {e}"))?;

    Ok(envelope.blocks.into_iter().find(|b| b.is_active && !b.is_gap))
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
            match poll_once(engine) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Real output of `ccusage v20 blocks --active --json` (captured 2026-07-16).
    /// Locks the serde model's contract against the real JSON.
    const REAL_SAMPLE: &str = r#"{
      "blocks": [{
        "actualEndTime": "2026-07-16T08:54:57.757Z",
        "burnRate": { "costPerHour": 16.81, "tokensPerMinute": 313145.96, "tokensPerMinuteForIndicator": 3093.26 },
        "costUSD": 24.846,
        "endTime": "2026-07-16T12:00:00.000Z",
        "entries": 262,
        "id": "2026-07-16T07:00:00.000Z",
        "isActive": true,
        "isGap": false,
        "models": ["claude-opus-4-8"],
        "projection": { "remainingMinutes": 185, "totalCost": 76.68, "totalTokens": 85701641 },
        "startTime": "2026-07-16T07:00:00.000Z",
        "tokenCounts": { "cacheCreationInputTokens": 544396, "cacheReadInputTokens": 26950933, "inputTokens": 46557, "outputTokens": 227752 },
        "totalTokens": 27769638
      }]
    }"#;

    #[test]
    fn parses_real_active_block() {
        let env: BlocksEnvelope = serde_json::from_str(REAL_SAMPLE).expect("must parse");
        let block = env
            .blocks
            .into_iter()
            .find(|b| b.is_active && !b.is_gap)
            .expect("there's an active block");
        assert_eq!(block.total_tokens, 27_769_638);
        assert_eq!(block.token_counts.output_tokens, 227_752);
        assert_eq!(block.projection.unwrap().remaining_minutes, 185);
        assert!(block.burn_rate.unwrap().cost_per_hour > 0.0);
    }

    /// A "gap" block omits burnRate/projection: it must not break parsing.
    #[test]
    fn tolerates_gap_block_missing_fields() {
        let json = r#"{"blocks":[{"id":"x","isGap":true,"isActive":false}]}"#;
        let env: BlocksEnvelope = serde_json::from_str(json).expect("gap parses");
        let b = &env.blocks[0];
        assert!(b.is_gap);
        assert!(b.burn_rate.is_none());
        assert!(b.projection.is_none());
    }

    /// Empty JSON / no blocks: valid envelope, zero blocks.
    #[test]
    fn tolerates_empty() {
        let env: BlocksEnvelope = serde_json::from_str("{}").expect("empty parses");
        assert!(env.blocks.is_empty());
    }
}
