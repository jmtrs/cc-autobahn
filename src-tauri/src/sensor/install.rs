//! Auto-install as statusLine (D12) — consent + backup + rollback.
//!
//! Mutates `${cfg}/settings.json`, which is Zod-strict in Claude Code: one bad
//! field leaves the user without config. That's why the round-trip uses
//! `serde_json::Value` (NEVER a typed struct — don't drop unknown fields), with
//! a non-overwriting 0600 backup, atomic tmp+rename write, and post-write
//! re-validation + rollback. The binary is COPIED to
//! `${cfg}/cc-autobahn/cc-autobahn-statusline` (stable path) instead of writing
//! `current_exe()`, which would be ephemeral under Gatekeeper translocation
//! (D-new-2).

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{
    backup_once, claude_config_dir, install_stable_binary, read_settings_for_install,
    read_settings_value, refresh_binary_if_stale, write_settings_atomic,
    write_settings_with_rollback, SETTINGS_WRITE_LOCK,
};

const STATUSLINE_BIN: &str = "cc-autobahn-statusline";
const BAK_SUFFIX: &str = ".cc-autobahn.bak";
const APP_KEY: &str = "cc-autobahn"; // settings["cc-autobahn"]
const PREV_KEY: &str = "prevStatusLine"; // settings["cc-autobahn"]["prevStatusLine"]

/// Installation state reported to the frontend.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SensorStatus {
    installed: bool, // statusLine points to our binary
    has_prev: bool,  // there's a previous statusLine saved (for rollback)
}

/// Installation preview (for the consent modal).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPreview {
    prev_status_line: Option<serde_json::Value>,
    new_command: String,
    backup_path: String,
}

/// Stable path for the binary copy (resolves translocation, D-new-2).
fn stable_bin_path(cfg: &Path) -> PathBuf {
    cfg.join("cc-autobahn").join(STATUSLINE_BIN)
}

/// The `statusLine` command we'll write to settings.json.
fn statusline_command(cfg: &Path) -> String {
    format!("\"{}\" statusline", stable_bin_path(cfg).display())
}

/// `#[tauri::command]` Is it installed and pointing to us?
#[tauri::command]
pub fn sensor_status() -> SensorStatus {
    let Some(v) = read_settings_value() else {
        return SensorStatus {
            installed: false,
            has_prev: false,
        };
    };
    let obj = v.as_object();
    let installed = obj
        .and_then(|o| o.get("statusLine"))
        .and_then(|sl| sl.get("command"))
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.contains(STATUSLINE_BIN));
    let has_prev = obj
        .and_then(|o| o.get(APP_KEY))
        .and_then(|a| a.get(PREV_KEY))
        .is_some();
    SensorStatus {
        installed,
        has_prev,
    }
}

/// `#[tauri::command]` Computes the preview without touching anything (for confirmation).
#[tauri::command]
pub fn sensor_preview_install() -> Result<InstallPreview, String> {
    let cfg = claude_config_dir().ok_or("could not resolve CLAUDE_CONFIG_DIR")?;
    let prev = read_settings_value()
        .as_ref()
        .and_then(|v| v.as_object())
        .and_then(|o| o.get("statusLine"))
        .cloned();
    Ok(InstallPreview {
        prev_status_line: prev,
        new_command: statusline_command(&cfg),
        backup_path: cfg
            .join(format!("settings.json{BAK_SUFFIX}"))
            .to_string_lossy()
            .to_string(),
    })
}

/// Transforms `settings` (Value) applying the install. Returns the previous
/// `statusLine` (to write the chain's `prev-statusline`). PURE → testable.
///
/// Idempotent: if the current `statusLine` ALREADY points to us, we do NOT
/// capture ourselves as `prev` (that would cause an infinite recursive chain at runtime).
fn apply_install(settings: &mut serde_json::Value, command: &str) -> Option<serde_json::Value> {
    let obj = settings.as_object_mut()?;
    let already_ours = obj
        .get("statusLine")
        .and_then(|sl| sl.get("command"))
        .and_then(|c| c.as_str())
        .is_some_and(|c| c.contains(STATUSLINE_BIN));
    let prev = if already_ours {
        obj.get(APP_KEY).and_then(|a| a.get(PREV_KEY)).cloned()
    } else {
        obj.get("statusLine").cloned()
    };
    obj.insert(APP_KEY.to_string(), serde_json::json!({ PREV_KEY: prev }));
    obj.insert(
        "statusLine".to_string(),
        serde_json::json!({ "type": "command", "command": command }),
    );
    prev
}

/// Transforms `settings` (Value) undoing the install. PURE → testable.
fn apply_uninstall(settings: &mut serde_json::Value) {
    let Some(obj) = settings.as_object_mut() else {
        return;
    };
    let prev = obj.get(APP_KEY).and_then(|a| a.get(PREV_KEY)).cloned();
    match prev {
        Some(p) if !p.is_null() => {
            obj.insert("statusLine".to_string(), p);
        }
        _ => {
            obj.remove("statusLine");
        }
    }
    obj.remove(APP_KEY);
}

/// `#[tauri::command]` Installs: backup → copy binary → rewrite settings → validate.
#[tauri::command]
pub fn install_sensor() -> Result<(), String> {
    let cfg = claude_config_dir().ok_or("could not resolve CLAUDE_CONFIG_DIR")?;
    let settings_path = cfg.join("settings.json");
    let backup_path = cfg.join(format!("settings.json{BAK_SUFFIX}"));
    let bin_dir = cfg.join("cc-autobahn");
    let bin_path = stable_bin_path(&cfg);

    // Serializes against `permission::install`'s own settings.json writer
    // (D42 review fix) — both self-installers target the same file, and an
    // unsynchronized read-modify-write on each side can drop the other's
    // change if both run close together (e.g. approving both consent
    // prompts back-to-back).
    let _guard = SETTINGS_WRITE_LOCK.lock().unwrap();

    // 1. Current settings ({} if it doesn't exist). Error if it exists but fails to parse.
    let mut settings = read_settings_for_install(
        &settings_path,
        "settings.json is not strict JSON (does it have comments?). Configure the statusline manually.",
    )?;

    // 2. 0600 backup, WITHOUT overwriting a pre-existing one (caveman pattern).
    backup_once(&settings_path, &backup_path)?;

    // 3. copy the binary to a stable path (D-new-2).
    install_stable_binary(&bin_path)?;

    // 4. transform settings (pure apply_install) and write the prev-statusline.
    let prev = apply_install(&mut settings, &statusline_command(&cfg));
    let prev_file = bin_dir.join("prev-statusline");
    match prev
        .as_ref()
        .and_then(|v| v.get("command"))
        .and_then(|c| c.as_str())
    {
        Some(cmd) => {
            let _ = fs::write(&prev_file, cmd);
        }
        None => {
            let _ = fs::remove_file(&prev_file); // no prev → chain uses default line
        }
    }

    // 5. atomic write (tmp+rename, 0600) + re-validation + rollback.
    write_settings_with_rollback(&settings_path, &backup_path, &settings)
}

/// Refreshes the installed statusline binary copy if it's stale (D36): the
/// consent modal in [`install_sensor`] only ever runs once, so a copy made
/// by an old release never learns about newer builds on its own — every
/// subsequent release would leave `statusLine` pointing at dead code. Runs
/// silently on a background thread at every GUI startup: same stable path,
/// no `settings.json` write, no re-consent (nothing a user would need to
/// approve twice).
pub fn refresh_if_stale() {
    std::thread::spawn(|| {
        let installed = sensor_status().installed;
        let Some(cfg) = claude_config_dir() else {
            return;
        };
        refresh_binary_if_stale(installed, stable_bin_path(&cfg));
    });
}

/// `#[tauri::command]` Uninstalls: restores prevStatusLine (or removes it).
#[tauri::command]
pub fn uninstall_sensor() -> Result<(), String> {
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
// Tests — auto-install: PURE transformation of settings.json (D12)
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_then_uninstall_roundtrip_with_caveman() {
        // settings with a previous statusLine (caveman) + an unrelated field.
        let mut s: serde_json::Value = serde_json::json!({
            "statusLine": { "type": "command", "command": "bash /Users/x/caveman-statusline.sh" },
            "permissions": { "allow": ["ed:x"] }
        });
        let original = s.clone();

        apply_install(&mut s, "\"/p/cc-autobahn-statusline\" statusline");
        // statusLine now points to our binary.
        assert!(s["statusLine"]["command"]
            .as_str()
            .unwrap()
            .contains("cc-autobahn-statusline"));
        // The previous one is saved for rollback/chain.
        assert_eq!(
            s["cc-autobahn"]["prevStatusLine"]["command"],
            original["statusLine"]["command"]
        );
        // Unrelated field PRESERVED (round-trip with Value, not a typed struct).
        assert_eq!(s["permissions"]["allow"][0], "ed:x");

        apply_uninstall(&mut s);
        // uninstall restores the original statusLine and removes our key.
        assert_eq!(s["statusLine"], original["statusLine"]);
        assert!(s.get("cc-autobahn").is_none());
        assert_eq!(s["permissions"]["allow"][0], "ed:x");
    }

    #[test]
    fn install_on_empty_settings_then_uninstall() {
        let mut s = serde_json::json!({});
        apply_install(&mut s, "\"/p/cc-autobahn-statusline\" statusline");
        assert!(s["statusLine"]["command"]
            .as_str()
            .unwrap()
            .contains("cc-autobahn-statusline"));
        // No previous statusLine → prevStatusLine is null.
        assert!(s["cc-autobahn"]["prevStatusLine"].is_null());
        apply_uninstall(&mut s);
        // No prev existed → uninstall removes statusLine (no leftover junk).
        assert!(s.get("statusLine").is_none());
        assert!(s.get("cc-autobahn").is_none());
    }

    #[test]
    fn reinstall_keeps_original_prev_no_loop() {
        // Already installed with a real prev (caveman). Reinstalling must NOT capture
        // itself as prev → this avoids an infinite recursive chain at runtime.
        let mut s = serde_json::json!({
            "statusLine": { "type": "command", "command": "\"/p/cc-autobahn-statusline\" statusline" },
            "cc-autobahn": { "prevStatusLine": { "type": "command", "command": "bash prev.sh" } }
        });
        apply_install(&mut s, "\"/p/cc-autobahn-statusline\" statusline");
        // The preserved prev is still the original one, NOT our own command.
        assert_eq!(
            s["cc-autobahn"]["prevStatusLine"]["command"],
            serde_json::json!("bash prev.sh")
        );
    }
}
