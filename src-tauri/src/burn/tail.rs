//! Tail of the active JSONL — I/O side of the `burn` sensor.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use super::parser::{process_line, split_lines, BurnCalc, TurnState};

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

/// Tails a single file: read position (`pos`) + remainder without `\n`
/// (`leftover`) so as not to lose partially-written lines. `pos` advances through all
/// bytes read; `leftover` is kept without re-reading (see `drain`).
pub(crate) struct Tail {
    pub(crate) active: Option<PathBuf>,
    pub(crate) pos: u64,
    pub(crate) leftover: Vec<u8>,
    state: TurnState,
}

impl Tail {
    pub(crate) fn new() -> Self {
        Tail {
            active: None,
            pos: 0,
            leftover: Vec::new(),
            state: TurnState::new(),
        }
    }

    /// Re-selects the most recent JSONL if the active session rotated. Call at
    /// low frequency (not every tick): it's a `readdir` over all projects.
    pub(crate) fn rescan(&mut self, projects_dir: &Path) {
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
    pub(crate) fn drain(&mut self) -> Vec<BurnCalc> {
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
    pub(crate) fn pump(&mut self, app: &AppHandle) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid assistant line for the parser.
    fn assistant(id: &str, ts: &str, out: u64, stop: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","stop_reason":"{stop}","usage":{{"output_tokens":{out}}}}}}}"#
        )
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
