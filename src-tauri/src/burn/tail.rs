//! Tail of all active JSONLs — I/O side of the `burn` sensor.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use super::parser::{process_line, split_lines, BurnCalc, TurnState};

/// A session file is tracked as long as it was written to within this many
/// seconds of "now". Decoupled from ccusage's 5h billing block on purpose
/// (`burn` has no dependency on `engine`, keeps D13's independent-threads
/// design) — a plain recency heuristic instead. It is only a discovery window:
/// once a file is already tailed, its state is retained while the file exists so
/// long quiet turns can still close later.
const ACTIVE_WINDOW_SECS: u64 = 60 * 60;

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
/// bytes read; `leftover` is kept without re-reading (see `drain`). The path
/// itself isn't stored here — `TailSet` owns it as the map key.
pub(crate) struct Tail {
    pub(crate) pos: u64,
    pub(crate) leftover: Vec<u8>,
    state: TurnState,
}

impl Tail {
    pub(crate) fn new() -> Self {
        Tail {
            pos: 0,
            leftover: Vec::new(),
            state: TurnState::new(),
        }
    }

    /// Drains the new bytes from `path` and returns the turns closed in this
    /// cycle. Does NOT emit — separates I/O from emission so it can be tested
    /// without `AppHandle`. Updates `pos`/`leftover`/state.
    pub(crate) fn drain(&mut self, path: &Path) -> Vec<BurnCalc> {
        let mut ticks = Vec::new();
        // Cheap stat BEFORE opening (D27 addendum): at 200 ms cadence, the
        // common case (nothing new written) shouldn't pay for an `open()`+`fstat` —
        // a `metadata()` without opening the file is enough to skip the cycle.
        let Ok(meta) = std::fs::metadata(path) else {
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

        let Ok(mut f) = OpenOptions::new().read(true).open(path) else {
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
    pub(crate) fn pump(&mut self, app: &AppHandle, path: &Path) {
        for calc in self.drain(path) {
            let _ = app.emit("burn-tick", BurnTick::from(calc));
        }
    }
}

/// All discovered JSONL files, each with its own tail state. Keyed by path for
/// cheap add-new/drop-deleted on rescan — replaces the old
/// single-`Tail`-by-highest-mtime model, which silently dropped every
/// concurrent Claude Code session but the most recently written one.
pub(crate) struct TailSet {
    tails: HashMap<PathBuf, Tail>,
}

impl TailSet {
    pub(crate) fn new() -> Self {
        TailSet {
            tails: HashMap::new(),
        }
    }

    /// Adds newly-discovered fresh files (new `Tail` starting at EOF, D8: zero
    /// historical noise) and drops files that were deleted. Call at low
    /// frequency (not every tick): it's a `readdir` over all projects.
    pub(crate) fn rescan(&mut self, projects_dir: &Path) {
        self.rescan_at(projects_dir, SystemTime::now());
    }

    fn rescan_at(&mut self, projects_dir: &Path, now: SystemTime) {
        let active = active_jsonls(projects_dir, now);
        let active_paths: std::collections::HashSet<&PathBuf> =
            active.iter().map(|(p, _)| p).collect();
        self.tails
            .retain(|p, _| active_paths.contains(p) || p.is_file());
        for (path, size) in active {
            self.tails.entry(path).or_insert_with(|| {
                let mut t = Tail::new();
                // EOF-start with the size from the SAME rescan metadata (avoids
                // a second racy `stat`) — same D8 rule as before, applied per file.
                t.pos = size;
                t
            });
        }
    }

    /// Drains and emits `burn-tick` for each closed turn, across every
    /// tracked file. Each `Tail` emits independently — no coalescing into a
    /// combined event (the frontend already treats `burn-tick` as
    /// last-write-wins plus a recent-ticks buffer, per D27's partial ticks).
    pub(crate) fn pump(&mut self, app: &AppHandle) {
        for (path, tail) in self.tails.iter_mut() {
            tail.pump(app, path);
        }
    }
}

/// Returns every regular `.jsonl` under `projects_dir/**/*.jsonl` whose mtime
/// is within `ACTIVE_WINDOW_SECS` of `now`, along with its size (from the
/// same `metadata`, no second `stat`). Walks by hand — no `walkdir`. Ignores
/// stray errors (never aborts). Requires a regular file (`is_file`) so as not
/// to swallow a directory named `*.jsonl`.
fn active_jsonls(projects_dir: &Path, now: SystemTime) -> Vec<(PathBuf, u64)> {
    let mut out = Vec::new();
    let Some(dirs) = fs_read_dir(projects_dir) else {
        return out;
    };
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
            let Ok(mtime) = meta.modified() else {
                continue;
            };
            let fresh = now
                .duration_since(mtime)
                .map(|age| age.as_secs() <= ACTIVE_WINDOW_SECS)
                .unwrap_or(true); // mtime "in the future" (clock skew) → treat as fresh
            if fresh {
                out.push((path, meta.len()));
            }
        }
    }
    out
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
        let path = std::env::temp_dir().join(format!(
            "cc-autobahn-burn-test-{}.jsonl",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let mut tail = Tail::new();
        tail.pos = 0;

        // Cycle 1: complete tool_use + end_turn WITHOUT a trailing '\n' (partial write).
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(
                f,
                "{}",
                assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use")
            )
            .unwrap();
            write!(
                f,
                "{}",
                assistant("a2", "2026-07-16T08:00:10.000Z", 200, "end_turn")
            )
            .unwrap();
        }
        let t1 = tail.drain(&path);
        assert!(
            t1.is_empty(),
            "without a trailing '\\n' → the turn doesn't close yet"
        );
        // The partial line (a2, without '\n') stays retained in leftover for the next
        // cycle; pos has already advanced to the end of the file.
        assert!(
            tail.leftover.windows(2).any(|w| w == b"a2"),
            "the leftover retains the partial line: {:?}",
            String::from_utf8_lossy(&tail.leftover)
        );

        // Cycle 2: the missing '\n' is added. The line must complete without duplicating.
        {
            use std::fs::OpenOptions;
            let mut f = OpenOptions::new().write(true).open(&path).unwrap();
            f.seek(SeekFrom::End(0)).unwrap();
            writeln!(f).unwrap();
        }
        let t2 = tail.drain(&path);
        assert_eq!(t2.len(), 1, "closes exactly once");
        // If the leftover had been duplicated, the line wouldn't parse → t2 empty.
        assert_eq!(t2[0].turn_output_tokens, 300); // 100 + 200
        assert!((t2[0].tok_per_s - 30.0).abs() < 1e-6); // 300 / 10 s

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn end_to_end_against_real_jsonl() {
        // Processes ALL `.jsonl` files available under `~/.claude/projects/`
        // (TurnState reset per file, same as the real tail when tracking
        // multiple concurrent sessions) and verifies the parser closes real
        // turns with tok/s > 0.
        // Doesn't depend on any specific project → portable and without filtering paths.
        let home = crate::env_lock::var_os("HOME").map(PathBuf::from);
        let Some(home) = home else { return }; // skip on machines without HOME
        let projects = home.join(".claude/projects");
        let Some(files) = collect_jsonl(&projects) else {
            return; // no logs → silent skip
        };

        let mut ticks: Vec<BurnCalc> = Vec::new();
        for path in &files {
            let Ok(data) = std::fs::read_to_string(path) else {
                continue;
            };
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
            assert!(
                c.tok_per_s.is_finite() && c.tok_per_s > 0.0,
                "invalid tok/s"
            );
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

    /// `active_jsonls` must exclude files older than `ACTIVE_WINDOW_SECS` and
    /// include fresh ones; `TailSet::rescan` uses that only for discovery, with
    /// new tails starting at EOF.
    #[test]
    fn active_jsonls_excludes_stale_includes_fresh() {
        let dir = std::env::temp_dir().join(format!(
            "cc-autobahn-tailset-test-{}-{}",
            std::process::id(),
            "freshness"
        ));
        let projects_dir = dir.join("proj");
        std::fs::create_dir_all(&projects_dir).unwrap();

        let fresh = projects_dir.join("fresh.jsonl");
        let stale = projects_dir.join("stale.jsonl");
        std::fs::write(&fresh, b"{}\n").unwrap();
        std::fs::write(&stale, b"{}\n").unwrap();

        let now = SystemTime::now();
        let found = active_jsonls(&dir, now);
        assert!(
            found.iter().any(|(p, _)| p == &fresh),
            "fresh file must be tracked"
        );
        assert!(
            found.iter().any(|(p, _)| p == &stale),
            "just-written file must be tracked before it ages out"
        );

        // Simulate the stale file having aged past the window: check it
        // straight instead of sleeping the test.
        let far_future = now + std::time::Duration::from_secs(ACTIVE_WINDOW_SECS + 60);
        let found_later = active_jsonls(&dir, far_future);
        assert!(
            found_later.is_empty(),
            "both files must have aged out of the window"
        );

        let mut set = TailSet::new();
        set.rescan(&dir);
        assert_eq!(set.tails.len(), 2, "rescan tracks both fresh files");
        for tail in set.tails.values() {
            assert_eq!(tail.pos, 3, "new Tail starts at EOF (D8), not 0");
        }

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Once a file is being tailed, aging past `ACTIVE_WINDOW_SECS` must not
    /// discard its state: a long-running turn can be quiet for more than the
    /// discovery window and still write the closing `end_turn` later.
    #[test]
    fn rescan_keeps_known_stale_file_state() {
        let dir = std::env::temp_dir().join(format!(
            "cc-autobahn-tailset-test-{}-{}",
            std::process::id(),
            "known-stale"
        ));
        let projects_dir = dir.join("proj");
        std::fs::create_dir_all(&projects_dir).unwrap();
        let path = projects_dir.join("quiet.jsonl");

        std::fs::write(
            &path,
            format!(
                "{}\n",
                assistant("q1", "2026-07-16T08:00:00.000Z", 100, "tool_use")
            ),
        )
        .unwrap();

        let mut set = TailSet::new();
        set.tails.insert(path.clone(), Tail::new());
        let first = set.tails.get_mut(&path).unwrap().drain(&path);
        assert!(
            first.is_empty(),
            "tool_use starts the turn but does not close it"
        );

        let far_future =
            SystemTime::now() + std::time::Duration::from_secs(ACTIVE_WINDOW_SECS + 60);
        set.rescan_at(&dir, far_future);
        assert!(
            set.tails.contains_key(&path),
            "known files stay tailed after aging out of discovery"
        );

        std::fs::write(
            &path,
            format!(
                "{}\n{}\n",
                assistant("q1", "2026-07-16T08:00:00.000Z", 100, "tool_use"),
                assistant("q2", "2026-07-16T08:30:00.000Z", 200, "end_turn")
            ),
        )
        .unwrap();

        let ticks = set.tails.get_mut(&path).unwrap().drain(&path);
        assert_eq!(ticks.len(), 1, "quiet long-running turn still closes");
        assert_eq!(ticks[0].turn_output_tokens, 300);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Two concurrent session files must be tailed independently: writing to
    /// one must not perturb the other's `pos`/state, and each emits its own
    /// closed turns.
    #[test]
    fn drain_isolates_concurrent_files() {
        let dir = std::env::temp_dir().join(format!(
            "cc-autobahn-tailset-test-{}-{}",
            std::process::id(),
            "isolation"
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path_a = dir.join("a.jsonl");
        let path_b = dir.join("b.jsonl");

        std::fs::write(
            &path_a,
            format!(
                "{}\n{}\n",
                assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use"),
                assistant("a2", "2026-07-16T08:00:10.000Z", 200, "end_turn")
            ),
        )
        .unwrap();
        std::fs::write(
            &path_b,
            format!(
                "{}\n{}\n",
                assistant("b1", "2026-07-16T09:00:00.000Z", 50, "tool_use"),
                assistant("b2", "2026-07-16T09:00:05.000Z", 150, "end_turn")
            ),
        )
        .unwrap();

        let mut tail_a = Tail::new();
        let mut tail_b = Tail::new();
        let ticks_a = tail_a.drain(&path_a);
        let ticks_b = tail_b.drain(&path_b);

        assert_eq!(ticks_a.len(), 1, "file A closes its own turn");
        assert_eq!(ticks_a[0].turn_output_tokens, 300);
        assert_eq!(ticks_b.len(), 1, "file B closes its own turn independently");
        assert_eq!(ticks_b[0].turn_output_tokens, 200);
        assert_ne!(
            tail_a.pos, tail_b.pos,
            "distinct files advance distinct positions"
        );

        // A second drain on A alone must not affect B's already-recorded pos.
        let pos_b_before = tail_b.pos;
        tail_a.drain(&path_a);
        assert_eq!(
            tail_b.pos, pos_b_before,
            "draining A doesn't touch B's state"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
