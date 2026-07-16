//! burn — `tok/s` **per response** sensor (D8).
//!
//! Tails the JSONL of the active session at `~/.claude/projects/**/*.jsonl`
//! and, when each turn closes, calculates `Δoutput / Δt_turn` and emits `burn-tick`.
//! It's the data ccusage doesn't offer — but it's **not instant**: the JSONL only
//! stamps `usage` when the message finishes (D8/DATA-ENGINE §Source 2), never
//! mid-generation (this CANNOT be sped up by polling faster: the data
//! simply doesn't exist on disk until that instant). In turns with
//! tool use (several `assistant` messages before closing), a PARTIAL
//! tick IS emitted for each intermediate message, in addition to the final
//! aggregate of the complete turn — earlier feedback without waiting for closure (D27).
//!
//! Sober design (no plugins, no async framework, no new crates): a
//! dedicated thread that does `stat` + `read` on the file every 200 ms. This isn't the waste
//! that D13 forbids (that was spawning Node per tick); a `stat` is a trivial
//! syscall. kqueue/inotify would require the `notify` crate — rejected per the
//! W203 principle of minimal pieces. The Zulu timestamp is parsed by hand (no `chrono`):
//! Claude Code's format is always `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC, `Z`).

mod parser;
mod tail;
mod zulu;

use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use tauri::AppHandle;

use tail::Tail;

/// Cadence of the JSONL `stat` (D13: event-driven in spirit; `stat` isn't
/// process spawning — opening+stat+reading-if-changed a single file every 200 ms
/// is negligible cost). Previously 1000 ms: it showed as perceptible lag between
/// the turn's actual closure and the needle reacting; 200 ms brings it down to
/// imperceptible without touching the cadence (5 s) for which file is active.
const BURN_TAIL_INTERVAL_MS: u64 = 200;
/// How often the most recent JSONL is re-searched for (the active session can rotate).
const ACTIVE_RESCAN_SECS: u64 = 5;

/// Starts the sensor in a dedicated thread. Looks for `~/.claude/projects/` and tails
/// the most recent JSONL, emitting `burn-tick` for each closed turn. Never
/// panics; any failure is silently ignored (it will be retried).
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        let Some(home) = crate::env_lock::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        let projects = home.join(".claude").join("projects");
        let mut tail = Tail::new();
        // Spaced-out re-scan: the `readdir` over all projects doesn't need to
        // run every tick. Drain every 1 s; re-scan every N ticks.
        let scan_every = (ACTIVE_RESCAN_SECS * 1000 / BURN_TAIL_INTERVAL_MS).max(1);
        let mut tick = 0u64;

        loop {
            if tick.is_multiple_of(scan_every) {
                tail.rescan(&projects);
            }
            tail.pump(&app);
            tick = tick.wrapping_add(1);
            thread::sleep(Duration::from_millis(BURN_TAIL_INTERVAL_MS));
        }
    });
}
