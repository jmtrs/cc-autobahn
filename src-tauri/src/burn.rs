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

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Cadence of the JSONL `stat` (D13: event-driven in spirit; `stat` isn't
/// process spawning — opening+stat+reading-if-changed a single file every 200 ms
/// is negligible cost). Previously 1000 ms: it showed as perceptible lag between
/// the turn's actual closure and the needle reacting; 200 ms brings it down to
/// imperceptible without touching the cadence (5 s) for which file is active.
const TAIL_INTERVAL_MS: u64 = 200;
/// How often the most recent JSONL is re-searched for (the active session can rotate).
const ACTIVE_RESCAN_SECS: u64 = 5;

// ─────────────────────────────────────────────────────────────────────────────
// Zulu timestamp → epoch-millis (no `chrono`)
// Claude Code's fixed format: "2026-07-16T08:34:42.592Z"
// ─────────────────────────────────────────────────────────────────────────────

/// Converts a Zulu timestamp to epoch-millis. `None` if the format doesn't match.
fn parse_zulu_millis(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 24 || b[23] != b'Z' {
        return None;
    }
    let n = |start: usize| -> Option<i64> {
        std::str::from_utf8(&b[start..start + 2])
            .ok()?
            .parse::<i64>()
            .ok()
    };
    let y: i64 = std::str::from_utf8(&b[0..4]).ok()?.parse().ok()?;
    let mo = n(5)?;
    let d = n(8)?;
    let hh = n(11)?;
    let mi = n(14)?;
    let ss = n(17)?;
    let msec: i64 = std::str::from_utf8(&b[20..23]).ok()?.parse().ok()?;
    // Defensive range validation (Claude Code writes valid values, but an
    // out-of-range field would silently produce an incorrect epoch_ms → None).
    if !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
        || !(0..=23).contains(&hh)
        || !(0..=59).contains(&mi)
        || !(0..=59).contains(&ss)
        || !(0..=999).contains(&msec)
    {
        return None;
    }
    let days = days_from_civil(y, mo as u64, d as u64);
    Some(
        days * 86_400_000
            + hh * 3_600_000
            + mi * 60_000
            + ss * 1000
            + msec,
    )
}

/// Days since 1970-01-01 for a civil date (Howard Hinnant's algorithm,
/// tested and branch-free). Proleptic Gregorian.
fn days_from_civil(y: i64, m: u64, d: u64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i64 - 719_468
}

// ─────────────────────────────────────────────────────────────────────────────
// Serde model of a JSONL line (only what we use)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct JsonlLine {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    message: Option<AssistantMsg>,
}

#[derive(Deserialize)]
struct AssistantMsg {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    output_tokens: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Turn calculation — PURE LOGIC (no Tauri) → testable
// ─────────────────────────────────────────────────────────────────────────────

/// Closure of a turn: ready to emit as `burn-tick`.
#[derive(Debug, Clone, PartialEq)]
struct BurnCalc {
    tok_per_s: f64,
    turn_output_tokens: u64,
    turn_duration_ms: i64,
    message_id: String,
    timestamp: String,
}

/// State of the current turn. Accumulates `output_tokens` (deduped by `message.id`)
/// from the previous closure until the next `end_turn`/`stop_sequence`.
#[derive(Default)]
struct TurnState {
    /// `ts` of the previous turn's closure (None at startup).
    last_end_ms: Option<i64>,
    /// Σ deduplicated `output_tokens` of the current turn.
    turn_output: u64,
    /// `ts` of the first accumulated message of the turn (Δt fallback if there's no
    /// previous closure — e.g. when hooking into a file mid-session).
    turn_start_ms: Option<i64>,
    /// `ts` of the last message seen (any, not just closures) — the base for the
    /// Δt of intermediate partial ticks. `None` when each turn starts.
    last_msg_ms: Option<i64>,
    /// `message.id`s already counted (discards rewrites, which carry the same value).
    seen: HashSet<String>,
}

impl TurnState {
    fn new() -> Self {
        Self::default()
    }

    /// Ingests an assistant message. Returns `Some(BurnCalc)` in two cases:
    /// (a) this message closes the turn (`end_turn`/`stop_sequence`, aggregate
    /// of the WHOLE turn) or (b) it's an intermediate message (`tool_use`, etc.) that
    /// isn't the first of the turn — PARTIAL tick with only its own
    /// tokens/Δt, so as not to wait for closure in long turns with tool use.
    fn ingest(
        &mut self,
        msg_id: &str,
        out_tok: u64,
        ts_ms: i64,
        stop_reason: Option<&str>,
        timestamp: &str,
    ) -> Option<BurnCalc> {
        // Tokens counted ONLY the first time we see the id (rewrites
        // carry the same value, D8). Closure is always processed: if an id seen
        // as `tool_use` reappears as `end_turn`, the turn must close regardless.
        let first_time = self.seen.insert(msg_id.to_string());
        if first_time {
            if self.turn_start_ms.is_none() {
                self.turn_start_ms = Some(ts_ms);
            }
            self.turn_output += out_tok;
        }

        let closes = matches!(stop_reason, Some("end_turn") | Some("stop_sequence"));

        if !closes {
            // Intermediate message: partial tick only if it's the first time we
            // see it (a rewrite isn't new work) and there's real Δt — the
            // turn's first message always has Δt=0 against itself, so
            // it doesn't emit (correct: there's nothing to measure yet).
            if !first_time || out_tok == 0 {
                return None;
            }
            let start_ms = self.last_msg_ms.or(self.turn_start_ms).unwrap_or(ts_ms);
            let dt_ms = ts_ms - start_ms;
            if dt_ms <= 0 {
                // ts not monotonic: do NOT seal last_msg_ms (same as last_end_ms on
                // closure) so as not to lose the reference for the next Δt.
                return None;
            }
            self.last_msg_ms = Some(ts_ms);
            return Some(BurnCalc {
                tok_per_s: out_tok as f64 * 1000.0 / dt_ms as f64,
                turn_output_tokens: out_tok,
                turn_duration_ms: dt_ms,
                message_id: msg_id.to_string(),
                timestamp: timestamp.to_string(),
            });
        }

        // `turn_output == 0` discards: empty turns (0 tokens) and rewrites of
        // an already-emitted closure (which would leave the accumulator at 0 after reset).
        if self.turn_output == 0 {
            return None;
        }

        // Δt = current closure − previous closure (or, failing that, turn start).
        let start_ms = self.last_end_ms.or(self.turn_start_ms).unwrap_or(ts_ms);
        let dt_ms = ts_ms - start_ms;
        if dt_ms <= 0 {
            // ts not monotonic (rewrite/clock glitch): don't emit OR seal the closure,
            // so as not to lose accumulated tokens or skew the next Δt.
            return None;
        }

        let calc = BurnCalc {
            tok_per_s: self.turn_output as f64 * 1000.0 / dt_ms as f64,
            turn_output_tokens: self.turn_output,
            turn_duration_ms: dt_ms,
            message_id: msg_id.to_string(),
            timestamp: timestamp.to_string(),
        };
        // Seal the closure ONLY when emitting: the next turn starts from here.
        self.last_end_ms = Some(ts_ms);
        self.turn_output = 0;
        self.turn_start_ms = None;
        self.last_msg_ms = None;
        Some(calc)
    }
}

/// Processes a raw JSONL line. `Some(BurnCalc)` if it closes a turn.
fn process_line(state: &mut TurnState, line: &[u8]) -> Option<BurnCalc> {
    let parsed: JsonlLine = serde_json::from_slice(line).ok()?;
    if parsed.kind != "assistant" {
        return None;
    }
    let msg = parsed.message?;
    let usage = msg.usage?;
    let msg_id = msg.id?;
    let ts_ms = parse_zulu_millis(&parsed.timestamp)?;
    state.ingest(
        &msg_id,
        usage.output_tokens,
        ts_ms,
        msg.stop_reason.as_deref(),
        &parsed.timestamp,
    )
}

/// Accumulates `chunk` into `leftover` and returns the complete lines (without `\n`),
/// leaving the remainder without `\n` in `leftover` for the next cycle. This way a chunk
/// that cuts a line halfway is reassembled when the next batch arrives.
fn split_lines(leftover: &mut Vec<u8>, chunk: &[u8]) -> Vec<Vec<u8>> {
    leftover.extend_from_slice(chunk);
    let bytes = std::mem::take(leftover);
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            out.push(bytes[start..i].to_vec());
            start = i + 1;
        }
    }
    *leftover = bytes[start..].to_vec();
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Event payload to the frontend
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct BurnTick {
    tok_per_s: f64,
    turn_output_tokens: u64,
    turn_duration_ms: i64,
    message_id: String,
    timestamp: String,
}

impl From<BurnCalc> for BurnTick {
    fn from(c: BurnCalc) -> Self {
        BurnTick {
            tok_per_s: c.tok_per_s,
            turn_output_tokens: c.turn_output_tokens,
            turn_duration_ms: c.turn_duration_ms,
            message_id: c.message_id,
            timestamp: c.timestamp,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tail of the active JSONL
// ─────────────────────────────────────────────────────────────────────────────

/// Tails a single file: read position (`pos`) + remainder without `\n`
/// (`leftover`) so as not to lose partially-written lines. `pos` advances through all
/// bytes read; `leftover` is kept without re-reading (see `drain`).
struct Tail {
    active: Option<PathBuf>,
    pos: u64,
    leftover: Vec<u8>,
    state: TurnState,
}

impl Tail {
    fn new() -> Self {
        Tail {
            active: None,
            pos: 0,
            leftover: Vec::new(),
            state: TurnState::new(),
        }
    }

    /// Re-selects the most recent JSONL if the active session rotated. Call at
    /// low frequency (not every tick): it's a `readdir` over all projects.
    fn rescan(&mut self, projects_dir: &Path) {
        if let Some((latest, size)) = most_recent_jsonl(projects_dir) {
            if self.active.as_deref() != Some(latest.as_path()) {
                self.active = Some(latest);
                // EOF-start with the size from the SAME rescan metadata (avoids a
                // second racy `stat`). Zero historical noise: the needle starts
                // at idle and reacts only to turns that close from now on (D8).
                self.pos = size;
                self.leftover.clear();
                self.state = TurnState::new();
            }
        }
    }

    /// Drains the new bytes from the active file and returns the turns closed
    /// in this cycle. Does NOT emit — separates I/O from emission so it can be tested without
    /// `AppHandle`. Updates `pos`/`leftover`/state.
    fn drain(&mut self) -> Vec<BurnCalc> {
        let mut ticks = Vec::new();
        let Some(path) = self.active.clone() else {
            return ticks;
        };
        // Cheap stat BEFORE opening (D27 addendum): at 200 ms cadence, the
        // common case (nothing new written) shouldn't pay for an `open()`+`fstat` —
        // a `metadata()` without opening the file is enough to skip the cycle.
        let Ok(meta) = std::fs::metadata(&path) else {
            return ticks;
        };
        let len = meta.len();

        // Truncation detected (the file shrank). Jump to the end — NEVER to 0:
        // re-reading from the start would re-emit historical burn-ticks (noise). State
        // reset because the file's context changed.
        if len < self.pos {
            self.pos = len;
            self.leftover.clear();
            self.state = TurnState::new();
        }
        if len <= self.pos {
            return ticks;
        }

        let Ok(mut f) = OpenOptions::new().read(true).open(&path) else {
            return ticks;
        };
        if f.seek(SeekFrom::Start(self.pos)).is_err() {
            return ticks;
        }
        let mut chunk = vec![0u8; (len - self.pos) as usize];
        let n = match f.read(&mut chunk) {
            Ok(n) => n,
            Err(_) => return ticks,
        };

        // Splits by lines; keeps the remainder without `\n` for the next cycle.
        for line in split_lines(&mut self.leftover, &chunk[..n]) {
            if line.is_empty() {
                continue;
            }
            if let Some(calc) = process_line(&mut self.state, &line) {
                ticks.push(calc);
            }
        }
        // We advance through ALL bytes read from the file (not just up to the last
        // `\n`): the remainder without `\n` is already in `leftover`, no need to re-read it.
        // (Previously we only advanced up to the last `\n` → the leftover got re-read and
        // duplicated, corrupting partial lines — see test `drain_partial_line`.)
        self.pos += n as u64;
        ticks
    }

    /// Drains and emits `burn-tick` for each closed turn.
    fn pump(&mut self, app: &AppHandle) {
        for calc in self.drain() {
            let _ = app.emit("burn-tick", BurnTick::from(calc));
        }
    }
}

/// Returns the regular `.jsonl` with the highest `mtime` under `projects_dir/**/*.jsonl`,
/// along with its size (from the same `metadata`, no second `stat`). Walks
/// by hand — no `walkdir`. Ignores stray errors (never aborts). Requires a
/// regular file (`is_file`) so as not to swallow a directory named `*.jsonl`.
fn most_recent_jsonl(projects_dir: &Path) -> Option<(PathBuf, u64)> {
    let dirs = fs_read_dir(projects_dir)?;
    let mut best: Option<(PathBuf, std::time::SystemTime, u64)> = None;
    for dir in dirs {
        let dir = match dir {
            Ok(d) => d.path(),
            Err(_) => continue,
        };
        for entry in fs_read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            if let Ok(mtime) = meta.modified() {
                if best.as_ref().is_none_or(|(_, t, _)| mtime > *t) {
                    best = Some((path, mtime, meta.len()));
                }
            }
        }
    }
    best.map(|(p, _, size)| (p, size))
}

fn fs_read_dir(p: &Path) -> Option<std::fs::ReadDir> {
    std::fs::read_dir(p).ok()
}

/// Starts the sensor in a dedicated thread. Looks for `~/.claude/projects/` and tails
/// the most recent JSONL, emitting `burn-tick` for each closed turn. Never
/// panics; any failure is silently ignored (it will be retried).
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        let projects = home.join(".claude").join("projects");
        let mut tail = Tail::new();
        // Spaced-out re-scan: the `readdir` over all projects doesn't need to
        // run every tick. Drain every 1 s; re-scan every N ticks.
        let scan_every = (ACTIVE_RESCAN_SECS * 1000 / TAIL_INTERVAL_MS).max(1);
        let mut tick = 0u64;

        loop {
            if tick.is_multiple_of(scan_every) {
                tail.rescan(&projects);
            }
            tail.pump(&app);
            tick = tick.wrapping_add(1);
            thread::sleep(Duration::from_millis(TAIL_INTERVAL_MS));
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests — against controlled cases and against the project's real JSONL.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zulu_epoch_origin() {
        assert_eq!(parse_zulu_millis("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_zulu_millis("1970-01-01T00:00:01.000Z"), Some(1000));
        assert_eq!(parse_zulu_millis("1970-01-02T00:00:00.000Z"), Some(86_400_000));
    }

    #[test]
    fn zulu_real_delta_matches_d8() {
        // The difference between the previous closure and the 3008-tok one (D8 case):
        // 1 min + 5 s + 278 ms = 65.278 s.
        let prev = parse_zulu_millis("2026-07-16T08:33:37.314Z").unwrap();
        let curr = parse_zulu_millis("2026-07-16T08:34:42.592Z").unwrap();
        assert_eq!(curr - prev, 65_278);
    }

    #[test]
    fn zulu_rejects_garbage() {
        assert_eq!(parse_zulu_millis("nope"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:34:42.592"), None); // missing Z
    }

    /// Minimal valid assistant line for the parser.
    fn assistant(id: &str, ts: &str, out: u64, stop: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","stop_reason":"{stop}","usage":{{"output_tokens":{out}}}}}}}"#
        )
    }

    #[test]
    fn turn_calc_first_turn_uses_turn_start() {
        // First turn: no previous closure, Δt = from the first message.
        let mut s = TurnState::new();
        let a = assistant("m1", "2026-07-16T08:00:00.000Z", 200, "tool_use");
        let b = assistant("m2", "2026-07-16T08:00:10.000Z", 300, "end_turn");
        assert!(process_line(&mut s, a.as_bytes()).is_none()); // tool_use doesn't close
        let calc = process_line(&mut s, b.as_bytes()).expect("closes turn");
        // Δoutput=500, Δt=10 s → 50 tok/s
        assert_eq!(calc.turn_output_tokens, 500);
        assert_eq!(calc.turn_duration_ms, 10_000);
        assert!((calc.tok_per_s - 50.0).abs() < 1e-6, "tok/s = {}", calc.tok_per_s);
    }

    #[test]
    fn turn_calc_second_turn_uses_last_end() {
        let mut s = TurnState::new();
        process_line(&mut s, assistant("m1", "2026-07-16T08:00:00.000Z", 200, "tool_use").as_bytes());
        process_line(&mut s, assistant("m2", "2026-07-16T08:00:10.000Z", 300, "end_turn").as_bytes());
        // Second turn: previous closure = 08:00:10.
        process_line(&mut s, assistant("m3", "2026-07-16T08:00:15.000Z", 100, "tool_use").as_bytes());
        let calc = process_line(&mut s, assistant("m4", "2026-07-16T08:00:35.000Z", 400, "end_turn").as_bytes())
            .expect("closes second turn");
        // Δoutput=500, Δt=08:00:35 − 08:00:10 = 25 s → 20 tok/s
        assert_eq!(calc.turn_output_tokens, 500);
        assert_eq!(calc.turn_duration_ms, 25_000);
        assert!((calc.tok_per_s - 20.0).abs() < 1e-6);
    }

    #[test]
    fn intermediate_tool_use_emits_partial_tick() {
        // Turn with 2 tool_use before closure: the FIRST has Δt=0 (turn
        // start, nothing to measure yet) but the SECOND does emit a
        // partial tick with ONLY its own tokens/Δt (not accumulated), without waiting
        // for final closure — earlier feedback in long turns with
        // tool use (D27).
        let mut s = TurnState::new();
        let a1 = assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use");
        let a2 = assistant("a2", "2026-07-16T08:00:05.000Z", 150, "tool_use");
        assert!(
            process_line(&mut s, a1.as_bytes()).is_none(),
            "first message of the turn, Δt=0 against itself"
        );
        let partial = process_line(&mut s, a2.as_bytes())
            .expect("second intermediate message emits partial tick");
        // Δoutput = 150 (ONLY a2, not accumulated with a1), Δt = 5 s → 30 tok/s
        assert_eq!(partial.turn_output_tokens, 150);
        assert_eq!(partial.turn_duration_ms, 5_000);
        assert!((partial.tok_per_s - 30.0).abs() < 1e-6);

        // The final closure DOES aggregate the WHOLE turn (100+150+200=450), not just
        // what remained since the last partial tick.
        let close = process_line(
            &mut s,
            assistant("a3", "2026-07-16T08:00:15.000Z", 200, "end_turn").as_bytes(),
        )
        .expect("closes turn");
        assert_eq!(close.turn_output_tokens, 450);
        assert_eq!(close.turn_duration_ms, 15_000); // from the start of the turn
        assert!((close.tok_per_s - 30.0).abs() < 1e-6); // 450 / 15
    }

    #[test]
    fn intermediate_dt_non_monotonic_does_not_seal_last_msg() {
        // Mirror of `dt_non_monotonic_does_not_reset` but for the
        // PARTIAL tick: an intermediate message with non-monotonic ts must not seal
        // `last_msg_ms`, or the next partial tick would calculate its Δt against
        // an incorrect reference (bug found in code review, D27).
        let mut s = TurnState::new();
        let a1 = assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use");
        assert!(process_line(&mut s, a1.as_bytes()).is_none(), "Δt=0, start of the turn");

        // a2 arrives with ts EARLIER than a1 (rewrite/clock glitch) → Δt<0 → None,
        // and must NOT seal last_msg_ms with this erroneous ts.
        let bad = assistant("a2", "2026-07-16T07:59:00.000Z", 50, "tool_use");
        assert!(process_line(&mut s, bad.as_bytes()).is_none(), "non-monotonic ts doesn't emit");

        // a3 arrives correctly 5s after a1 (NOT after a2): if last_msg_ms had
        // been sealed with a2's ts, Δt would be absurdly large.
        let a3 = assistant("a3", "2026-07-16T08:00:05.000Z", 150, "tool_use");
        let partial = process_line(&mut s, a3.as_bytes())
            .expect("partial tick uses the correct reference (a1, not a2)");
        assert_eq!(partial.turn_duration_ms, 5_000); // 08:00:05 − 08:00:00, not vs 07:59:00
        assert!((partial.tok_per_s - 30.0).abs() < 1e-6); // 150 tok / 5 s
    }

    #[test]
    fn dedup_by_message_id() {
        // Rewrite of the same message.id → not counted twice.
        let mut s = TurnState::new();
        let first = assistant("m1", "2026-07-16T08:00:00.000Z", 200, "tool_use");
        let rewrite = assistant("m1", "2026-07-16T08:00:01.000Z", 200, "tool_use");
        assert!(process_line(&mut s, first.as_bytes()).is_none());
        assert!(process_line(&mut s, rewrite.as_bytes()).is_none()); // ignored
        let calc = process_line(&mut s, assistant("m2", "2026-07-16T08:00:10.000Z", 300, "end_turn").as_bytes())
            .unwrap();
        // 200 (once) + 300 = 500, not 700.
        assert_eq!(calc.turn_output_tokens, 500);
    }

    #[test]
    fn ignores_non_assistant_and_partial() {
        let mut s = TurnState::new();
        // user / system / garbage → none close.
        assert!(process_line(&mut s, br#"{"type":"user","timestamp":"2026-07-16T08:00:00.000Z"}"#).is_none());
        assert!(process_line(&mut s, b"this is not json").is_none());
        assert!(process_line(&mut s, b"").is_none());
        // assistant without usage → ignored.
        assert!(process_line(&mut s, br#"{"type":"assistant","timestamp":"2026-07-16T08:00:00.000Z","message":{"id":"x"}}"#).is_none());
    }

    #[test]
    fn zulu_rejects_out_of_range() {
        // hour 24, minute 60, etc. → None (no silently incorrect epoch).
        assert_eq!(parse_zulu_millis("2026-07-16T24:00:00.000Z"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:60:00.000Z"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:00:60.000Z"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:00:00.9999Z"), None); // format
    }

    #[test]
    fn split_lines_handles_partial_writes() {
        let mut leftover = Vec::new();
        // chunk 1: "abc\ndef" → "def" stays as leftover (without \n).
        let l1 = split_lines(&mut leftover, b"abc\ndef");
        assert_eq!(l1, vec![b"abc".to_vec()]);
        assert_eq!(leftover, b"def");
        // chunk 2: "\nghi" → completes "def" WITHOUT duplicating it.
        let l2 = split_lines(&mut leftover, b"\nghi");
        assert_eq!(l2, vec![b"def".to_vec()]);
        assert_eq!(leftover, b"ghi");
    }

    #[test]
    fn dedup_does_not_swallow_closure() {
        // BUG 2: an id seen as tool_use and rewritten as end_turn must NOT
        // ignore the closure. Tokens are counted the first time (100).
        let mut s = TurnState::new();
        process_line(&mut s, assistant("mx", "2026-07-16T08:00:00.000Z", 100, "tool_use").as_bytes());
        let calc = process_line(
            &mut s,
            assistant("mx", "2026-07-16T08:00:10.000Z", 200, "end_turn").as_bytes(),
        )
        .expect("the closure must not be swallowed");
        assert_eq!(calc.turn_output_tokens, 100); // counted only once
    }

    #[test]
    fn dt_non_monotonic_does_not_reset() {
        // RISK 6: a closure with non-monotonic ts doesn't emit, does NOT update
        // last_end_ms (doesn't go backwards), and does NOT lose accumulated tokens.
        let mut s = TurnState::new();
        // Initial VALID turn (with prior tool_use → dt>0) to fix last_end_ms.
        process_line(&mut s, assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use").as_bytes());
        process_line(&mut s, assistant("a2", "2026-07-16T08:00:10.000Z", 50, "end_turn").as_bytes());
        // Turn in progress: tool_use accumulates 300.
        process_line(&mut s, assistant("t1", "2026-07-16T08:00:15.000Z", 300, "tool_use").as_bytes());
        // closure with ts EARLIER than the last valid closure (08:00:10) → dt < 0 → None.
        let bad = process_line(
            &mut s,
            assistant("c2", "2026-07-16T07:59:00.000Z", 0, "stop_sequence").as_bytes(),
        );
        assert!(bad.is_none(), "non-monotonic ts doesn't emit");
        // correct later closure: Δt uses the original last_end_ms (08:00:10), not 07:59:00.
        let good = process_line(
            &mut s,
            assistant("c3", "2026-07-16T08:00:30.000Z", 10, "end_turn").as_bytes(),
        )
        .expect("closes with tokens preserved");
        assert_eq!(good.turn_duration_ms, 20_000); // 08:00:30 − 08:00:10, not 90 s
        assert_eq!(good.turn_output_tokens, 310); // 300 (t1) + 10 (c3); c2 contributed 0
    }

    /// BUG 1 (duplicated leftover regression): a line written halfway in
    /// one cycle must be completed and processed EXACTLY ONCE in the next,
    /// with no corruption or replay. Uses a real tmpfile.
    #[test]
    fn drain_partial_line_not_duplicated() {
        use std::io::{Seek, Write};
        let path =
            std::env::temp_dir().join(format!("cc-autobahn-burn-test-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut tail = Tail::new();
        tail.active = Some(path.clone());
        tail.pos = 0;

        // Cycle 1: complete tool_use + end_turn WITHOUT a trailing '\n' (partial write).
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, "{}", assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use")).unwrap();
            write!(f, "{}", assistant("a2", "2026-07-16T08:00:10.000Z", 200, "end_turn")).unwrap();
        }
        let t1 = tail.drain();
        assert!(t1.is_empty(), "without a trailing '\\n' → the turn doesn't close yet");
        // The partial line (a2, without '\n') stays retained in leftover for the next
        // cycle; pos has already advanced to the end of the file.
        assert!(tail.leftover.windows(2).any(|w| w == b"a2"),
            "the leftover retains the partial line: {:?}",
            String::from_utf8_lossy(&tail.leftover));

        // Cycle 2: the missing '\n' is added. The line must complete without duplicating.
        {
            use std::fs::OpenOptions;
            let mut f = OpenOptions::new().write(true).open(&path).unwrap();
            f.seek(SeekFrom::End(0)).unwrap();
            write!(f, "\n").unwrap();
        }
        let t2 = tail.drain();
        assert_eq!(t2.len(), 1, "closes exactly once");
        // If the leftover had been duplicated, the line wouldn't parse → t2 empty.
        assert_eq!(t2[0].turn_output_tokens, 300); // 100 + 200
        assert!((t2[0].tok_per_s - 30.0).abs() < 1e-6); // 300 / 10 s

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn end_to_end_against_real_jsonl() {
        // Processes ALL `.jsonl` files available under `~/.claude/projects/`
        // (TurnState reset per file, same as the real tail when rotating
        // sessions) and verifies the parser closes real turns with tok/s > 0.
        // Doesn't depend on any specific project → portable and without filtering paths.
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let Some(home) = home else { return }; // skip on machines without HOME
        let projects = home.join(".claude/projects");
        let Some(files) = collect_jsonl(&projects) else {
            return; // no logs → silent skip
        };

        let mut ticks: Vec<BurnCalc> = Vec::new();
        for path in &files {
            let Ok(data) = std::fs::read_to_string(path) else { continue };
            let mut s = TurnState::new(); // fresh per session, like the tail
            for line in data.lines() {
                if let Some(c) = process_line(&mut s, line.as_bytes()) {
                    ticks.push(c);
                }
            }
        }
        if ticks.is_empty() {
            return; // no session with closed turns → skip
        }
        // Every emitted tick has finite, positive tok/s.
        for c in &ticks {
            assert!(c.tok_per_s.is_finite() && c.tok_per_s > 0.0, "invalid tok/s");
            assert!(c.turn_output_tokens > 0, "turn without output");
        }
    }

    /// Collects all regular `.jsonl` files under `projects/*/*.jsonl`, sorted.
    fn collect_jsonl(projects: &Path) -> Option<Vec<PathBuf>> {
        let mut files: Vec<PathBuf> = Vec::new();
        for dir in std::fs::read_dir(projects).ok()?.flatten() {
            let dir = dir.path();
            for entry in std::fs::read_dir(&dir).ok()?.flatten() {
                let path = entry.path();
                if path.extension().and_then(|x| x.to_str()) == Some("jsonl")
                    && entry.metadata().map(|m| m.is_file()).unwrap_or(false)
                {
                    files.push(path);
                }
            }
        }
        files.sort();
        Some(files)
    }
}
