//! PATH hardening (D36): an app launched from Finder/Dock inherits launchd's
//! bare PATH (typically `/usr/bin:/bin:/usr/sbin:/sbin`), which hides the
//! tooling the engine cascade looks for (D9: Homebrew's `npx`/`bunx`,
//! `~/.bun/bin`, …). The dev flow never noticed — `npm run tauri dev`
//! inherits the terminal's PATH. Prepend the usual suspects that exist on
//! disk so `engine::detect` sees what the user actually has installed.
//!
//! Runs once at startup, GUI mode only (statusline mode parses stdin and
//! spawns nothing).

use std::ffi::OsString;
use std::path::PathBuf;

/// Directories prepended when they exist on disk, in priority order.
fn candidates() -> Vec<PathBuf> {
    let mut dirs = vec![
        PathBuf::from("/opt/homebrew/bin"), // Homebrew, Apple Silicon
        PathBuf::from("/usr/local/bin"),    // Homebrew, Intel / node .pkg
    ];
    if let Some(home) = crate::env_lock::var_os("HOME") {
        let home = PathBuf::from(home);
        dirs.push(home.join(".bun/bin")); // official Bun installer (D9)
        dirs.push(home.join(".local/bin"));
    }
    dirs
}

/// PURE: `existing` PATH with the on-disk, not-already-present candidates
/// prepended (order preserved, no duplicates). Testable.
pub(crate) fn hardened(existing: Option<OsString>, candidates: &[PathBuf]) -> OsString {
    let existing_dirs: Vec<PathBuf> = existing
        .as_deref()
        .map(|p| std::env::split_paths(p).collect())
        .unwrap_or_default();
    // Candidates win the front of PATH even if already present further back
    // (that's the point of hardening it) — dedup by dropping their old spot.
    let winners: Vec<PathBuf> = candidates.iter().filter(|d| d.is_dir()).cloned().collect();
    let mut dirs = winners.clone();
    dirs.extend(existing_dirs.into_iter().filter(|d| !winners.contains(d)));
    match std::env::join_paths(dirs) {
        Ok(joined) => joined,
        Err(_) => existing.unwrap_or_default(), // unquotable dir: leave PATH untouched
    }
}

/// Reads PATH, prepends the candidates, writes it back (under the env lock).
pub fn apply() {
    let current = crate::env_lock::var_os("PATH");
    let next = hardened(current, &candidates());
    crate::env_lock::set_var("PATH", next);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ccab-pathfix-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn prepends_existing_candidates_and_skips_missing_and_dupes() {
        let present = temp_dir("present");
        let missing = std::env::temp_dir().join("ccab-pathfix-definitely-not-there");
        let already = temp_dir("already");
        let existing = std::env::join_paths([&already, &present]).unwrap();
        // `present` is both in PATH and a candidate → must appear exactly once,
        // and the candidate copy wins the order (prepended).
        let out = hardened(
            Some(existing),
            &[present.clone(), missing.clone(), already.clone()],
        );
        let dirs: Vec<PathBuf> = std::env::split_paths(&out).collect();
        assert_eq!(dirs[0], present);
        assert_eq!(dirs.iter().filter(|d| **d == present).count(), 1);
        assert!(!dirs.contains(&missing));
        assert!(dirs.contains(&already));
        std::fs::remove_dir_all(&present).ok();
        std::fs::remove_dir_all(&already).ok();
    }

    #[test]
    fn no_home_no_candidates_keeps_existing_untouched() {
        let existing = OsString::from("/usr/bin:/bin");
        let out = hardened(Some(existing.clone()), &[]);
        assert_eq!(out, existing);
    }
}
