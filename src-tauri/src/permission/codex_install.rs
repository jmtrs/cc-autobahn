//! User-layer Codex `PermissionRequest` hook install. Codex discovers
//! `${CODEX_HOME:-~/.codex}/hooks.json`; unrelated events and handlers are
//! preserved. Trust remains Codex-owned and is reported separately by
//! `hooks/list`.

use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::{AppHandle, Manager};

use crate::sensor::{
    backup_once, install_stable_binary, read_settings_for_install, read_settings_value_at,
    refresh_binary_if_stale, write_settings_atomic, write_settings_with_rollback,
    SETTINGS_WRITE_LOCK,
};

const HOOK_BIN: &str = "cc-autobahn-codex-permission-hook";
const BAK_SUFFIX: &str = ".cc-autobahn.bak";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexPermissionStatus {
    configured_locally: bool,
    installed: bool,
    enabled: Option<bool>,
    trust_status: Option<String>,
    active: bool,
    last_observed_at_ms: Option<i64>,
    source_path: Option<String>,
    current_hash: Option<String>,
    inventory_observed_at_ms: Option<i64>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexInstallPreview {
    new_command: String,
    backup_path: String,
    existing_hook_count: usize,
    hooks_path: String,
}

pub(crate) fn codex_config_dir() -> Option<PathBuf> {
    if let Some(value) = crate::env_lock::var_os("CODEX_HOME") {
        let first = value
            .to_string_lossy()
            .split(',')
            .map(str::trim)
            .find(|entry| !entry.is_empty())?
            .to_string();
        let path = PathBuf::from(first);
        if path.extension().and_then(|extension| extension.to_str()) == Some("jsonl") {
            return None;
        }
        return Some(path);
    }
    crate::env_lock::var_os("HOME").map(|home| PathBuf::from(home).join(".codex"))
}

pub(crate) fn hooks_path() -> Option<PathBuf> {
    Some(codex_config_dir()?.join("hooks.json"))
}

fn stable_bin_path(config: &Path) -> PathBuf {
    config.join("cc-autobahn").join(HOOK_BIN)
}

pub(crate) fn hook_command(config: &Path) -> String {
    format!(
        "\"{}\" permission-hook codex",
        stable_bin_path(config).display()
    )
}

fn is_our_entry(entry: &serde_json::Value, command: &str) -> bool {
    let Some(hooks) = entry.get("hooks").and_then(serde_json::Value::as_array) else {
        return false;
    };
    entry.get("matcher").and_then(serde_json::Value::as_str) == Some("*")
        && hooks.len() == 1
        && hooks[0].get("type").and_then(serde_json::Value::as_str) == Some("command")
        && hooks[0].get("command").and_then(serde_json::Value::as_str) == Some(command)
}

fn is_configured(path: &Path, command: &str) -> bool {
    read_settings_value_at(path)
        .as_ref()
        .and_then(|value| value.get("hooks"))
        .and_then(|hooks| hooks.get("PermissionRequest"))
        .and_then(serde_json::Value::as_array)
        .is_some_and(|entries| entries.iter().any(|entry| is_our_entry(entry, command)))
}

#[tauri::command]
pub fn codex_permission_status(app: AppHandle) -> Result<CodexPermissionStatus, String> {
    let path = hooks_path().ok_or("could not resolve a writable Codex user config layer")?;
    let probe = app
        .state::<crate::providers::codex::app_server::AccountSensorState>()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .permission_hook
        .clone();
    let activity = app
        .state::<super::PermissionActivityState>()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(&crate::providers::ProviderId::Codex)
        .cloned();
    let active = activity_matches_probe(probe.as_ref(), activity.as_ref());
    Ok(CodexPermissionStatus {
        configured_locally: is_configured(
            &path,
            &hook_command(path.parent().unwrap_or_else(|| Path::new(""))),
        ),
        installed: probe.is_some(),
        enabled: probe.as_ref().map(|probe| probe.enabled),
        trust_status: probe.as_ref().map(|probe| probe.trust_status.clone()),
        active,
        last_observed_at_ms: activity.map(|activity| activity.observed_at_ms),
        source_path: probe.as_ref().map(|probe| probe.source_path.clone()),
        current_hash: probe.as_ref().and_then(|probe| probe.current_hash.clone()),
        inventory_observed_at_ms: app
            .state::<crate::providers::codex::app_server::AccountSensorState>()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .permission_hook_observed_at_ms,
    })
}

pub(crate) fn activity_matches_probe(
    probe: Option<&crate::providers::codex::app_server::CodexHookProbe>,
    activity: Option<&super::PermissionActivity>,
) -> bool {
    match (probe, activity) {
        (Some(probe), Some(activity)) => {
            probe.enabled
                && probe.trust_status == "trusted"
                && probe.current_hash.is_some()
                && activity.hook_hash == probe.current_hash
        }
        _ => false,
    }
}

#[tauri::command]
pub fn codex_permission_preview_install() -> Result<CodexInstallPreview, String> {
    let config =
        codex_config_dir().ok_or("could not resolve a writable Codex user config layer")?;
    let path = config.join("hooks.json");
    let existing_hook_count = read_settings_value_at(&path)
        .as_ref()
        .and_then(|value| value.get("hooks"))
        .and_then(|hooks| hooks.get("PermissionRequest"))
        .and_then(serde_json::Value::as_array)
        .map(|entries| {
            let command = hook_command(&config);
            entries
                .iter()
                .filter(|entry| !is_our_entry(entry, &command))
                .count()
        })
        .unwrap_or_default();
    Ok(CodexInstallPreview {
        new_command: hook_command(&config),
        backup_path: config
            .join(format!("hooks.json{BAK_SUFFIX}"))
            .to_string_lossy()
            .into_owned(),
        existing_hook_count,
        hooks_path: path.to_string_lossy().into_owned(),
    })
}

fn apply_install(settings: &mut serde_json::Value, command: &str) {
    let Some(root) = settings.as_object_mut() else {
        return;
    };
    let hooks = root.entry("hooks").or_insert_with(|| serde_json::json!({}));
    let Some(hooks) = hooks.as_object_mut() else {
        return;
    };
    let entries = hooks
        .entry("PermissionRequest")
        .or_insert_with(|| serde_json::json!([]));
    let Some(entries) = entries.as_array_mut() else {
        return;
    };
    let entry = serde_json::json!({
        "matcher": "*",
        "hooks": [{
            "type": "command",
            "command": command,
            "timeout": 600,
            "statusMessage": "Waiting for cc-autobahn approval"
        }]
    });
    match entries
        .iter()
        .position(|entry| is_our_entry(entry, command))
    {
        Some(index) => entries[index] = entry,
        None => entries.push(entry),
    }
}

fn validate_hooks_shape(settings: &serde_json::Value) -> Result<(), String> {
    let root = settings
        .as_object()
        .ok_or("hooks.json root must be a JSON object")?;
    let Some(hooks) = root.get("hooks") else {
        return Ok(());
    };
    let hooks = hooks
        .as_object()
        .ok_or("hooks.json `hooks` must be a JSON object")?;
    let Some(entries) = hooks.get("PermissionRequest") else {
        return Ok(());
    };
    let entries = entries
        .as_array()
        .ok_or("hooks.json `hooks.PermissionRequest` must be an array")?;
    for (group_index, group) in entries.iter().enumerate() {
        let group = group.as_object().ok_or_else(|| {
            format!("hooks.json PermissionRequest group {group_index} must be an object")
        })?;
        if group
            .get("matcher")
            .is_some_and(|matcher| !matcher.is_string())
        {
            return Err(format!(
                "hooks.json PermissionRequest group {group_index} matcher must be a string"
            ));
        }
        let handlers = group
            .get("hooks")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| {
                format!("hooks.json PermissionRequest group {group_index} hooks must be an array")
            })?;
        for (handler_index, handler) in handlers.iter().enumerate() {
            let handler = handler.as_object().ok_or_else(|| {
                format!(
                    "hooks.json PermissionRequest group {group_index} handler {handler_index} must be an object"
                )
            })?;
            let handler_type = handler
                .get("type")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    format!(
                        "hooks.json PermissionRequest group {group_index} handler {handler_index} type must be a string"
                    )
                })?;
            if handler_type == "command"
                && handler
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .is_none()
            {
                return Err(format!(
                    "hooks.json PermissionRequest group {group_index} handler {handler_index} command must be a string"
                ));
            }
        }
    }
    Ok(())
}

fn apply_uninstall(settings: &mut serde_json::Value, command: &str) {
    let Some(root) = settings.as_object_mut() else {
        return;
    };
    let Some(hooks) = root
        .get_mut("hooks")
        .and_then(serde_json::Value::as_object_mut)
    else {
        return;
    };
    let Some(entries) = hooks
        .get_mut("PermissionRequest")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return;
    };
    entries.retain(|entry| !is_our_entry(entry, command));
    if entries.is_empty() {
        hooks.remove("PermissionRequest");
    }
    if hooks.is_empty() {
        root.remove("hooks");
    }
}

#[tauri::command]
pub fn install_codex_permission_hook(app: AppHandle) -> Result<(), String> {
    let config =
        codex_config_dir().ok_or("could not resolve a writable Codex user config layer")?;
    let path = config.join("hooks.json");
    let backup = config.join(format!("hooks.json{BAK_SUFFIX}"));
    let binary = stable_bin_path(&config);
    let _guard = SETTINGS_WRITE_LOCK.lock().unwrap();
    let existed = path.exists();
    let mut settings = read_settings_for_install(
        &path,
        "hooks.json is not strict JSON; configure the Codex hook manually",
    )?;
    validate_hooks_shape(&settings)?;
    let original = settings.clone();
    backup_once(&path, &backup)?;
    install_stable_binary(&binary)?;
    apply_install(&mut settings, &hook_command(&config));
    write_settings_with_rollback(&path, &backup, &settings)?;
    if is_configured(&path, &hook_command(&config)) {
        invalidate_probe(&app);
        return Ok(());
    }
    if existed {
        write_settings_atomic(&path, &original.to_string())?;
    } else {
        let _ = std::fs::remove_file(&path);
    }
    Err("Codex hook was not present after writing; previous configuration restored".into())
}

#[tauri::command]
pub fn uninstall_codex_permission_hook(app: AppHandle) -> Result<(), String> {
    let path = hooks_path().ok_or("could not resolve a writable Codex user config layer")?;
    let _guard = SETTINGS_WRITE_LOCK.lock().unwrap();
    if !path.exists() {
        invalidate_probe(&app);
        return Ok(());
    }
    let mut settings = read_settings_for_install(
        &path,
        "hooks.json is not strict JSON; remove the Codex hook manually",
    )?;
    validate_hooks_shape(&settings)?;
    let config = path
        .parent()
        .ok_or("invalid Codex hooks.json path")?
        .to_path_buf();
    let command = hook_command(&config);
    apply_uninstall(&mut settings, &command);
    write_settings_atomic(&path, &settings.to_string())?;
    invalidate_probe(&app);
    Ok(())
}

fn invalidate_probe(app: &AppHandle) {
    let state = app.state::<crate::providers::codex::app_server::AccountSensorState>();
    crate::providers::codex::app_server::invalidate_hook_probe(&state);
    app.state::<super::PermissionActivityState>()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&crate::providers::ProviderId::Codex);
}

pub fn refresh_if_stale() {
    std::thread::spawn(|| {
        let Some(config) = codex_config_dir() else {
            return;
        };
        let installed = is_configured(&config.join("hooks.json"), &hook_command(&config));
        refresh_binary_if_stale(installed, stable_bin_path(&config));
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn merge_and_uninstall_preserve_other_hooks() {
        let mut value = serde_json::json!({
            "description": "mine",
            "hooks": {
                "PermissionRequest": [{
                    "matcher": "Bash",
                    "hooks": [{"type": "command", "command": "./other"}]
                }],
                "PostToolUse": []
            }
        });
        apply_install(
            &mut value,
            "\"/tmp/cc-autobahn-codex-permission-hook\" permission-hook codex",
        );
        assert_eq!(
            value["hooks"]["PermissionRequest"]
                .as_array()
                .unwrap()
                .len(),
            2
        );
        apply_uninstall(
            &mut value,
            "\"/tmp/cc-autobahn-codex-permission-hook\" permission-hook codex",
        );
        assert_eq!(value["description"], "mine");
        assert_eq!(
            value["hooks"]["PermissionRequest"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(value["hooks"].get("PostToolUse").is_some());
    }

    #[test]
    fn reinstall_is_idempotent() {
        let mut value = serde_json::json!({});
        let command = "\"/tmp/cc-autobahn-codex-permission-hook\" permission-hook codex";
        apply_install(&mut value, command);
        apply_install(&mut value, command);
        let entries = value["hooks"]["PermissionRequest"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert!(entries[0]["hooks"][0]["command"]
            .as_str()
            .unwrap()
            .starts_with("\"/tmp/"));
    }

    #[test]
    fn ownership_does_not_match_commands_that_only_mention_binary_name() {
        let command = "\"/tmp/cc-autobahn-codex-permission-hook\" permission-hook codex";
        let wrapper = serde_json::json!({
            "matcher": "*",
            "hooks": [{
                "type": "command",
                "command": "sh -c 'echo cc-autobahn-codex-permission-hook'"
            }]
        });
        assert!(!is_our_entry(&wrapper, command));
    }

    #[test]
    fn invalid_hook_container_is_rejected() {
        assert!(validate_hooks_shape(&serde_json::json!([])).is_err());
        assert!(validate_hooks_shape(&serde_json::json!({ "hooks": [] })).is_err());
        assert!(validate_hooks_shape(&serde_json::json!({
            "hooks": { "PermissionRequest": {} }
        }))
        .is_err());
        assert!(validate_hooks_shape(&serde_json::json!({
            "hooks": { "PermissionRequest": ["bad"] }
        }))
        .is_err());
        assert!(validate_hooks_shape(&serde_json::json!({
            "hooks": {
                "PermissionRequest": [{
                    "matcher": "*",
                    "hooks": [{ "type": "command", "command": 42 }]
                }]
            }
        }))
        .is_err());
    }

    #[test]
    fn activity_is_valid_only_for_the_current_trusted_hash() {
        let probe = crate::providers::codex::app_server::CodexHookProbe {
            enabled: true,
            trust_status: "trusted".into(),
            source_path: "/tmp/hooks.json".into(),
            current_hash: Some("hash-2".into()),
            observed_at_ms: 10,
        };
        let current = crate::permission::PermissionActivity {
            observed_at_ms: 11,
            hook_hash: Some("hash-2".into()),
        };
        let stale = crate::permission::PermissionActivity {
            observed_at_ms: 9,
            hook_hash: Some("hash-1".into()),
        };
        assert!(activity_matches_probe(Some(&probe), Some(&current)));
        assert!(!activity_matches_probe(Some(&probe), Some(&stale)));
        assert!(!activity_matches_probe(None, Some(&current)));
    }
}
