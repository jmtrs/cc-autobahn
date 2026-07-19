//! Auto-install as a Claude Code `PermissionRequest` hook (D42) — consent +
//! backup + rollback, same shape as `sensor::install` (D12): `settings.json`
//! is round-tripped as `serde_json::Value` (never a typed struct — Claude
//! Code validates it with a strict Zod schema, one bad field bricks it),
//! non-overwriting 0600 backup, atomic tmp+rename write, post-write
//! re-validation with rollback. The binary is COPIED to a stable path
//! instead of writing `current_exe()`, for the same reason as the statusline
//! sensor: an unnotarized macOS `.app` runs from an ephemeral
//! Gatekeeper-translocation path.
//!
//! Differs from `statusLine` in one structural way: `hooks.PermissionRequest`
//! is an ARRAY of matcher-groups (other tools may already have entries
//! there, or under other event types), not a single object, so install must
//! merge without clobbering anyone else's hooks, and uninstall never needs a
//! "previous value" chain — it just removes the one matcher-group we added.

use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::sensor::{
    backup_once, claude_config_dir, install_stable_binary, read_settings_for_install,
    read_settings_value, refresh_binary_if_stale, write_settings_atomic,
    write_settings_with_rollback, SETTINGS_WRITE_LOCK,
};

const HOOK_BIN: &str = "cc-autobahn-permission-hook";
const BAK_SUFFIX: &str = ".cc-autobahn.bak";

/// Installation state reported to the frontend.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionStatus {
    installed: bool,
}

/// Installation preview (for the consent overlay).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPreview {
    new_command: String,
    backup_path: String,
    existing_hook_count: usize,
}

fn stable_bin_path(cfg: &Path) -> PathBuf {
    cfg.join("cc-autobahn").join(HOOK_BIN)
}

fn hook_command(cfg: &Path) -> String {
    format!("\"{}\" permission-hook", stable_bin_path(cfg).display())
}

/// `true` if this matcher-group's command points at our binary.
fn is_our_entry(entry: &serde_json::Value) -> bool {
    entry
        .get("hooks")
        .and_then(|hs| hs.as_array())
        .is_some_and(|hs| {
            hs.iter().any(|h| {
                h.get("command")
                    .and_then(|c| c.as_str())
                    .is_some_and(|c| c.contains(HOOK_BIN))
            })
        })
}

/// `#[tauri::command]` Is it installed and pointing to us?
#[tauri::command]
pub fn permission_status() -> PermissionStatus {
    let installed = read_settings_value()
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("hooks"))
        .and_then(|h| h.get("PermissionRequest"))
        .and_then(|arr| arr.as_array())
        .is_some_and(|arr| arr.iter().any(is_our_entry));
    PermissionStatus { installed }
}

/// `#[tauri::command]` Computes the preview without touching anything.
#[tauri::command]
pub fn permission_preview_install() -> Result<InstallPreview, String> {
    let cfg = claude_config_dir().ok_or("could not resolve CLAUDE_CONFIG_DIR")?;
    let existing_hook_count = read_settings_value()
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("hooks"))
        .and_then(|h| h.get("PermissionRequest"))
        .and_then(|arr| arr.as_array())
        .map(|arr| arr.iter().filter(|e| !is_our_entry(e)).count())
        .unwrap_or(0);
    Ok(InstallPreview {
        new_command: hook_command(&cfg),
        backup_path: cfg
            .join(format!("settings.json{BAK_SUFFIX}"))
            .to_string_lossy()
            .to_string(),
        existing_hook_count,
    })
}

/// Transforms `settings` (Value) applying the install — appends into the
/// `hooks.PermissionRequest` array, preserving every other matcher-group and
/// every other hook event type untouched. Idempotent: reinstalling replaces
/// our own entry in place instead of appending a duplicate. PURE → testable.
fn apply_install(settings: &mut serde_json::Value, command: &str) {
    let Some(obj) = settings.as_object_mut() else {
        return;
    };
    let hooks = obj.entry("hooks").or_insert_with(|| serde_json::json!({}));
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return;
    };
    let arr = hooks_obj
        .entry("PermissionRequest")
        .or_insert_with(|| serde_json::json!([]));
    let Some(arr) = arr.as_array_mut() else {
        return;
    };

    let entry = serde_json::json!({
        "matcher": "*",
        "hooks": [{ "type": "command", "command": command, "timeout": 600 }]
    });

    match arr.iter().position(is_our_entry) {
        Some(idx) => arr[idx] = entry,
        None => arr.push(entry),
    }
}

/// Transforms `settings` (Value) undoing the install — removes only the one
/// matcher-group we added, leaves every other entry (and every other hook
/// event type) intact. Cleans up empty `PermissionRequest`/`hooks` keys
/// rather than leaving `[]`/`{}` litter. PURE → testable.
fn apply_uninstall(settings: &mut serde_json::Value) {
    let Some(obj) = settings.as_object_mut() else {
        return;
    };
    let Some(hooks) = obj.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return;
    };
    let Some(arr) = hooks
        .get_mut("PermissionRequest")
        .and_then(|a| a.as_array_mut())
    else {
        return;
    };

    arr.retain(|e| !is_our_entry(e));

    if arr.is_empty() {
        hooks.remove("PermissionRequest");
    }
    if hooks.is_empty() {
        obj.remove("hooks");
    }
}

/// `#[tauri::command]` Installs: backup → copy binary → rewrite settings → validate.
#[tauri::command]
pub fn install_permission_hook() -> Result<(), String> {
    let cfg = claude_config_dir().ok_or("could not resolve CLAUDE_CONFIG_DIR")?;
    let settings_path = cfg.join("settings.json");
    let backup_path = cfg.join(format!("settings.json{BAK_SUFFIX}"));
    let bin_path = stable_bin_path(&cfg);

    // Serializes against `sensor::install`'s own settings.json writer (D42
    // review fix) — see that lock's doc comment.
    let _guard = SETTINGS_WRITE_LOCK.lock().unwrap();

    // 1. Current settings ({} if it doesn't exist). Error if it exists but fails to parse.
    let mut settings = read_settings_for_install(
        &settings_path,
        "settings.json is not strict JSON (does it have comments?). Configure the hook manually.",
    )?;

    // 2. 0600 backup, WITHOUT overwriting a pre-existing one (shared with the
    //    statusline sensor's backup — one canonical pre-cc-autobahn snapshot).
    backup_once(&settings_path, &backup_path)?;

    // 3. copy the binary to a stable path (Gatekeeper translocation, D12/D-new-2).
    install_stable_binary(&bin_path)?;

    // 4. transform settings (pure apply_install).
    apply_install(&mut settings, &hook_command(&cfg));

    // 5. atomic write (tmp+rename, 0600) + re-validation + rollback.
    write_settings_with_rollback(&settings_path, &backup_path, &settings)
}

/// Refreshes the installed permission-hook binary copy if it's stale (same
/// self-healing as `sensor::install::refresh_if_stale`, D36): the consent
/// flow in [`install_permission_hook`] only ever runs once, so without this
/// an old release's copy would keep pointing at dead code across upgrades.
/// Runs silently on a background thread at every GUI startup — no
/// `settings.json` write, no re-consent.
pub fn refresh_if_stale() {
    std::thread::spawn(|| {
        let installed = permission_status().installed;
        let Some(cfg) = claude_config_dir() else {
            return;
        };
        refresh_binary_if_stale(installed, stable_bin_path(&cfg));
    });
}

/// `#[tauri::command]` Uninstalls: removes our matcher-group only.
#[tauri::command]
pub fn uninstall_permission_hook() -> Result<(), String> {
    let cfg = claude_config_dir().ok_or("could not resolve CLAUDE_CONFIG_DIR")?;
    let settings_path = cfg.join("settings.json");
    let _guard = SETTINGS_WRITE_LOCK.lock().unwrap();
    let Some(mut settings) = read_settings_value() else {
        return Ok(()); // nothing to undo
    };
    apply_uninstall(&mut settings);
    write_settings_atomic(&settings_path, &settings.to_string())?;
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests — PURE transformation of settings.json (D42)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_on_empty_settings_then_uninstall() {
        let mut s = serde_json::json!({});
        apply_install(&mut s, "\"/p/cc-autobahn-permission-hook\" permission-hook");

        let arr = s["hooks"]["PermissionRequest"].as_array().unwrap();
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["matcher"], "*");
        assert!(arr[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("cc-autobahn-permission-hook"));

        apply_uninstall(&mut s);
        assert!(
            s.get("hooks").is_none(),
            "empty hooks object must be cleaned up, no [] / {{}} litter"
        );
    }

    #[test]
    fn install_preserves_other_hooks() {
        let mut s = serde_json::json!({
            "hooks": {
                "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "./block-rm.sh" }] }],
                "PermissionRequest": [{ "matcher": "Write", "hooks": [{ "type": "command", "command": "some-other-tool" }] }]
            }
        });
        apply_install(&mut s, "\"/p/cc-autobahn-permission-hook\" permission-hook");

        // Unrelated event type untouched.
        assert_eq!(s["hooks"]["PreToolUse"][0]["matcher"], "Bash");

        let arr = s["hooks"]["PermissionRequest"].as_array().unwrap();
        assert_eq!(
            arr.len(),
            2,
            "our entry appended, third-party entry preserved"
        );
        assert_eq!(arr[0]["hooks"][0]["command"], "some-other-tool");
        assert!(arr[1]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .contains("cc-autobahn-permission-hook"));

        apply_uninstall(&mut s);
        let arr = s["hooks"]["PermissionRequest"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "only our entry removed");
        assert_eq!(arr[0]["hooks"][0]["command"], "some-other-tool");
        assert_eq!(
            s["hooks"]["PreToolUse"][0]["matcher"], "Bash",
            "still untouched"
        );
    }

    #[test]
    fn reinstall_replaces_in_place_no_duplicate() {
        let mut s = serde_json::json!({});
        apply_install(
            &mut s,
            "\"/old/cc-autobahn-permission-hook\" permission-hook",
        );
        apply_install(
            &mut s,
            "\"/new/cc-autobahn-permission-hook\" permission-hook",
        );

        let arr = s["hooks"]["PermissionRequest"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "reinstall must not duplicate the entry");
        assert!(arr[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .starts_with("\"/new/"));
    }

    #[test]
    fn uninstall_when_never_installed_is_noop() {
        let mut s = serde_json::json!({ "hooks": { "PreToolUse": [] } });
        let before = s.clone();
        apply_uninstall(&mut s);
        assert_eq!(s, before);
    }
}
