//! "Always Allow" (D42 follow-up) — mirrors Claude Code's OWN "Yes, don't
//! ask again" semantics exactly, which are NOT one mechanism (confirmed
//! against code.claude.com/docs/en/permissions.md):
//!
//! - **Bash**: persists PERMANENTLY as a literal `Bash(<exact command>)`
//!   rule in `.claude/settings.local.json` **at the git repository root**
//!   (or `cwd` itself, outside a repo). Deliberate scope limit: exact-match
//!   only — no compound-command splitting / wrapper-stripping / wildcard
//!   generation like Claude Code's own generator does. Revisit only if a
//!   real need for prefix/wildcard rules shows up.
//! - **Read/Edit/Write**: lasts only "until session end" on Claude Code's
//!   own side, held in memory, never written to disk. This module's
//!   equivalent is an in-memory [`AlwaysAllowSet`] keyed by `session_id`,
//!   held by the GUI (the only long-running participant — the hook process
//!   is short-lived per call). "Session end" here is approximated as "this
//!   GUI process's lifetime for that session_id", since the hook has no
//!   visibility into Claude Code's real session lifecycle — an accepted,
//!   documented gap, not a silent one.
//!
//! Kept as its own file rather than growing `mod.rs` further: this owns a
//! distinct concern (per-repo settings mutation + session memory) from
//! `mod.rs` (queue/socket mechanics), `hook_bin.rs` (CLI side), and
//! `install.rs` (global `~/.claude/settings.json` self-install).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use tauri::{AppHandle, Manager};

use crate::providers::ProviderId;
use crate::sensor::{read_settings_for_install, write_settings_atomic, SETTINGS_WRITE_LOCK};

/// `(provider, session_id, tool_name, exact_input)` tuples marked Always-Allow.
/// Codex always uses this process-local session cache; Claude uses it for
/// non-Bash compatibility fallback and briefly after a persisted Bash rule.
/// Tauri-managed state, sibling to `PendingQueue`.
pub(crate) type AlwaysAllowSet = Arc<Mutex<HashSet<(ProviderId, String, String, String)>>>;

pub(crate) fn new_set() -> AlwaysAllowSet {
    Arc::new(Mutex::new(HashSet::new()))
}

/// `true` if `(session_id, tool_name, matched)` was previously marked
/// Always-Allow for a non-Bash tool in this GUI process's lifetime.
pub(crate) fn is_remembered(
    set: &AlwaysAllowSet,
    provider: ProviderId,
    session_id: &str,
    tool_name: &str,
    matched: &str,
) -> bool {
    set.lock().unwrap().contains(&(
        provider,
        session_id.to_string(),
        tool_name.to_string(),
        matched.to_string(),
    ))
}

/// Runs `git rev-parse --show-toplevel` with `cwd` as the working directory.
/// `None` on any failure (git missing, non-zero exit, not a repo, io error)
/// — impure (spawns a process), covered by the manual E2E script rather
/// than a unit test.
fn git_toplevel(cwd: &Path) -> Option<PathBuf> {
    let out = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(cwd)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?;
    let trimmed = s.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(PathBuf::from(trimmed))
    }
}

/// Where the Bash "Always Allow" rule is written, given an already-resolved
/// git toplevel (or `None` if `cwd` isn't inside a repo / git isn't
/// available). PURE → testable without touching the filesystem or spawning
/// git: mirrors documented Claude Code behavior — inside a repo, the rule
/// goes in the repo root's `.claude/settings.local.json`; outside one, it
/// goes in `cwd`'s own `.claude/settings.local.json` ("the directory you
/// started it from", approximated here as the hook request's own `cwd`).
pub(crate) fn local_settings_path(cwd: &Path, toplevel: Option<&Path>) -> PathBuf {
    toplevel
        .unwrap_or(cwd)
        .join(".claude")
        .join("settings.local.json")
}

fn bash_settings_path(cwd: &Path) -> PathBuf {
    local_settings_path(cwd, git_toplevel(cwd).as_deref())
}

/// Appends `Bash(<command>)` into `permissions.allow`, idempotent (no
/// duplicate on repeat approval), preserving every other key untouched —
/// same shape as `install::apply_install`. PURE → testable.
pub(crate) fn apply_bash_allow_rule(settings: &mut serde_json::Value, command: &str) {
    let Some(obj) = settings.as_object_mut() else {
        return;
    };
    let permissions = obj
        .entry("permissions")
        .or_insert_with(|| serde_json::json!({}));
    let Some(permissions_obj) = permissions.as_object_mut() else {
        return;
    };
    let allow = permissions_obj
        .entry("allow")
        .or_insert_with(|| serde_json::json!([]));
    let Some(allow) = allow.as_array_mut() else {
        return;
    };
    let rule = serde_json::Value::String(format!("Bash({command})"));
    if !allow.contains(&rule) {
        allow.push(rule);
    }
}

/// Persists an Always-Allow decision, dispatching on `tool_name` per the
/// module doc's Bash-vs-session split. Called from `permission_approve_always`
/// (mod.rs) after the matched field has already been resolved by the
/// caller.
pub(crate) fn remember(
    app: &AppHandle,
    provider: ProviderId,
    session_id: &str,
    tool_name: &str,
    cwd: &str,
    matched: &str,
) -> Result<(), String> {
    let remember_in_memory = || {
        let set = app.state::<AlwaysAllowSet>();
        set.lock().unwrap().insert((
            provider,
            session_id.to_string(),
            tool_name.to_string(),
            matched.to_string(),
        ));
    };

    if provider == ProviderId::Codex || tool_name != "Bash" {
        remember_in_memory();
        return Ok(());
    }

    let path = bash_settings_path(Path::new(cwd));

    // Serializes against every other settings-file writer in the process
    // (D42 review fix) — two concurrent "Always Allow" approvals for the
    // same repo, on separate connection threads, would otherwise race a
    // read-modify-write here and silently drop one rule.
    let _guard = SETTINGS_WRITE_LOCK.lock().unwrap();

    // Error (not silently treat-as-empty) if the file exists but fails to
    // parse — this is a per-repo settings.local.json a human may have
    // hand-edited; overwriting it with just the new rule would discard
    // every other key it holds.
    let mut settings = read_settings_for_install(
        &path,
        &format!(
            "{} is not strict JSON; resolve it manually before Always Allow can persist a rule there.",
            path.display()
        ),
    )?;
    apply_bash_allow_rule(&mut settings, matched);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("create_dir {}: {e}", parent.display()))?;
    }
    write_settings_atomic(&path, &settings.to_string())?;

    // Belt-and-suspenders while Claude Code reloads the persisted rule. This
    // happens only AFTER a successful write: a failed Always Allow must never
    // leave an invisible in-memory auto-approval active for future commands.
    remember_in_memory();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apply_bash_allow_rule_on_empty_settings() {
        let mut s = serde_json::json!({});
        apply_bash_allow_rule(&mut s, "npm test");
        let arr = s["permissions"]["allow"].as_array().unwrap();
        assert_eq!(arr, &vec![serde_json::json!("Bash(npm test)")]);
    }

    #[test]
    fn apply_bash_allow_rule_idempotent() {
        let mut s = serde_json::json!({});
        apply_bash_allow_rule(&mut s, "npm test");
        apply_bash_allow_rule(&mut s, "npm test");
        let arr = s["permissions"]["allow"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "repeat approval must not duplicate the rule");
    }

    #[test]
    fn apply_bash_allow_rule_preserves_existing_entries() {
        let mut s = serde_json::json!({
            "permissions": {
                "allow": ["Bash(git status)"],
                "deny": ["Bash(rm -rf *)"]
            }
        });
        apply_bash_allow_rule(&mut s, "npm test");
        let allow = s["permissions"]["allow"].as_array().unwrap();
        assert_eq!(allow.len(), 2);
        assert!(allow.contains(&serde_json::json!("Bash(git status)")));
        assert!(allow.contains(&serde_json::json!("Bash(npm test)")));
        assert_eq!(
            s["permissions"]["deny"],
            serde_json::json!(["Bash(rm -rf *)"]),
            "unrelated deny rules untouched"
        );
    }

    #[test]
    fn apply_bash_allow_rule_two_distinct_commands() {
        let mut s = serde_json::json!({});
        apply_bash_allow_rule(&mut s, "npm test");
        apply_bash_allow_rule(&mut s, "npm run build");
        let arr = s["permissions"]["allow"].as_array().unwrap();
        assert_eq!(arr.len(), 2);
    }

    #[test]
    fn local_settings_path_uses_toplevel_when_present() {
        let cwd = Path::new("/home/user/repo/sub/dir");
        let toplevel = Path::new("/home/user/repo");
        assert_eq!(
            local_settings_path(cwd, Some(toplevel)),
            PathBuf::from("/home/user/repo/.claude/settings.local.json")
        );
    }

    #[test]
    fn local_settings_path_falls_back_to_cwd() {
        let cwd = Path::new("/home/user/scratch");
        assert_eq!(
            local_settings_path(cwd, None),
            PathBuf::from("/home/user/scratch/.claude/settings.local.json")
        );
    }
}
