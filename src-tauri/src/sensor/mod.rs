//! sensor — OFFICIAL data via Claude Code's statusLine (D12).
//!
//! cc-autobahn is, in addition to the window, the `statusLine` command that Claude Code
//! invokes, passing it the session JSON via stdin — the ONLY source of official
//! `rate_limits` (5h / 7d window). The same binary works in two modes:
//!   · `cc-autobahn statusline` → reads stdin, re-emits the user's previous line
//!     (chain, D-new-3) and dumps the JSON to a file that the window tails
//!     (see [`statusline_bin`]).
//!   · no args → GUI mode (decided by `main` before building the webview).
//!
//! Sober design like `burn`/`engine`: zero new crates, dedicated thread with
//! `stat` + `read` every 2s (D13). Careful: `resets_at` in the JSON is epoch in SECONDS,
//! not Zulu ms — kept raw (does NOT reuse `burn::parse_zulu_millis`).

pub mod install;
pub mod statusline_bin;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

pub use statusline_bin::run_statusline;

/// Cadence of the sensor file `stat`. Not a process spawn (D13).
const SENSOR_TAIL_INTERVAL_MS: u64 = 2000;
/// If the sensor file hasn't refreshed in longer than this → sensor "disconnected".
const STALE_SECS: u64 = 60;

// ─────────────────────────────────────────────────────────────────────────────
// Claude Code config directory (CLAUDE_CONFIG_DIR or ~/.claude)
// ─────────────────────────────────────────────────────────────────────────────

/// Resolves `${CLAUDE_CONFIG_DIR:-$HOME/.claude}`. Single source of truth: used
/// by statusline mode (write), the tail (read), and install.
pub(crate) fn claude_config_dir() -> Option<PathBuf> {
    if let Some(dir) = crate::env_lock::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = crate::env_lock::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".claude"))
}

/// `~/.claude/cc-autobahn-status.json` — dumped by statusline mode, tailed
/// by [`start`].
pub(crate) fn status_file() -> Option<PathBuf> {
    Some(claude_config_dir()?.join("cc-autobahn-status.json"))
}

/// `~/.claude/cc-autobahn/prev-statusline` — the user's previous statusLine command,
/// for the chain (D-new-3) and for uninstall.
pub(crate) fn prev_statusline_file() -> Option<PathBuf> {
    Some(
        claude_config_dir()?
            .join("cc-autobahn")
            .join("prev-statusline"),
    )
}

/// Writes `buf` to `path` with mode 0600 (no partial-permission window on unix).
#[cfg(unix)]
pub(crate) fn write_private(path: &std::path::Path, buf: &[u8]) -> bool {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut f| f.write_all(buf).map(|_| f))
        .is_ok()
}

#[cfg(not(unix))]
pub(crate) fn write_private(path: &std::path::Path, buf: &[u8]) -> bool {
    fs::write(path, buf).is_ok()
}

// ─────────────────────────────────────────────────────────────────────────────
// Shared settings.json install helpers — every self-installer that mutates
// `${cfg}/settings.json` (this module's own statusLine install, D12, and
// `permission::install`'s PermissionRequest hook install, D42) needs the
// exact same backup/atomic-write/rollback safety mechanics. Kept here rather
// than duplicated per feature, found by code review (D42) after the first
// two installers turned out byte-for-byte identical on this part.
// ─────────────────────────────────────────────────────────────────────────────

pub(crate) fn settings_path() -> Option<PathBuf> {
    Some(claude_config_dir()?.join("settings.json"))
}

/// Serializes every read-modify-write of a settings JSON file across the
/// process: `sensor::install`/`permission::install` both target the same
/// `${cfg}/settings.json` (D42 review fix — two installers racing on the same
/// file, e.g. approving both self-install prompts back-to-back, could
/// otherwise drop one's change), and `permission::always_allow` does the same
/// dance against a per-repo `settings.local.json`. One global lock is
/// intentional over a per-path registry: these are human-paced actions
/// (install clicks, approval clicks), never a hot path, so the extra
/// serialization across unrelated paths costs nothing observable.
pub(crate) static SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());

/// Step 1 of every self-installer's shared shape: `{}` if `path` doesn't
/// exist yet, the parsed value if it does, or `msg` as the error if it
/// exists but isn't strict JSON (Claude Code's own settings.json schema is
/// Zod-strict — refusing to guess here, unlike overwriting it as if it were
/// empty, is what keeps a malformed file from silently losing its content).
pub(crate) fn read_settings_for_install(
    path: &Path,
    msg: &str,
) -> Result<serde_json::Value, String> {
    if !path.exists() {
        return Ok(serde_json::json!({}));
    }
    let data =
        fs::read_to_string(path).map_err(|e| format!("could not read {}: {e}", path.display()))?;
    serde_json::from_str(&data).map_err(|_| msg.to_string())
}

/// Step 2: non-overwriting 0600 backup of `path` at `backup_path`.
pub(crate) fn backup_once(path: &Path, backup_path: &Path) -> Result<(), String> {
    if path.exists() && !backup_path.exists() {
        copy_private(path, backup_path).map_err(|e| format!("backup failed: {e}"))?;
    }
    Ok(())
}

/// Step 3: copies the running binary to `dst` (a stable path, resolving
/// Gatekeeper translocation, D-new-2) with 0755 perms.
pub(crate) fn install_stable_binary(dst: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create_dir: {e}"))?;
    }
    fs::copy(&exe, dst).map_err(|e| format!("copy bin: {e}"))?;
    chmod_755(dst);
    Ok(())
}

/// Step 5: atomic write (tmp+rename, 0600) + post-write re-validation, with
/// rollback to `backup_path` if the write somehow left invalid JSON behind.
pub(crate) fn write_settings_with_rollback(
    settings_path: &Path,
    backup_path: &Path,
    settings: &serde_json::Value,
) -> Result<(), String> {
    write_settings_atomic(settings_path, &settings.to_string())?;
    let valid = fs::read_to_string(settings_path)
        .ok()
        .and_then(|d| serde_json::from_str::<serde_json::Value>(&d).ok())
        .is_some();
    if valid {
        Ok(())
    } else {
        if backup_path.exists() {
            let _ = fs::rename(backup_path, settings_path);
        }
        Err("settings invalid after writing; backup has been restored".to_string())
    }
}

/// Shared body of every self-installer's silent binary self-refresh (D36):
/// checked at every GUI startup on a dedicated thread, replaces the stable
/// binary copy only when it differs from the running exe. No settings.json
/// write, no re-consent — `installed`/`stable_bin_path` are the only bits
/// that differ per feature.
pub(crate) fn refresh_binary_if_stale(installed: bool, stable_bin_path: PathBuf) {
    if !installed {
        return; // not installed yet — the consent flow owns the first copy
    }
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    if same_contents(&exe, &stable_bin_path) {
        return;
    }
    if fs::copy(&exe, &stable_bin_path).is_ok() {
        chmod_755(&stable_bin_path);
    }
}

/// Reads and parses a settings JSON file at an arbitrary path as a `Value`.
/// `None` if it doesn't exist or fails to parse. Also backs
/// `permission::always_allow`'s per-repo `.claude/settings.local.json`
/// (D42) — not just `${cfg}/settings.json` — hence taking a path instead of
/// being hardcoded like `read_settings_value` below.
pub(crate) fn read_settings_value_at(path: &std::path::Path) -> Option<serde_json::Value> {
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Reads and parses `${cfg}/settings.json` specifically. `None` if it
/// doesn't exist or fails to parse.
pub(crate) fn read_settings_value() -> Option<serde_json::Value> {
    read_settings_value_at(&settings_path()?)
}

/// Writes `bytes` to `path` via atomic tmp+rename with mode 0600.
pub(crate) fn write_settings_atomic(path: &std::path::Path, bytes: &str) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    if !write_private(&tmp, bytes.as_bytes()) {
        return Err("could not write settings.json".to_string());
    }
    fs::rename(&tmp, path).map_err(|e| format!("rename settings: {e}"))
}

#[cfg(unix)]
pub(crate) fn chmod_755(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
pub(crate) fn chmod_755(_path: &std::path::Path) {}

#[cfg(unix)]
pub(crate) fn copy_private(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::copy(src, dst)?;
    let mut perms = fs::metadata(dst)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(dst, perms)?;
    Ok(())
}

#[cfg(not(unix))]
pub(crate) fn copy_private(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    fs::copy(src, dst)?;
    Ok(())
}

/// Byte-for-byte comparison — mtime isn't a reliable staleness signal here
/// (an app bundle replaced on disk can carry an older mtime than the copy
/// made from a previous, newer run).
pub(crate) fn same_contents(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (fs::read(a), fs::read(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Serde model of the statusLine JSON (snake_case, everything optional)
// Structured against the official docs + Wangnov/claude-code-statusline-pro.
// `resets_at` = epoch in SECONDS (i64).
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct StatusInput {
    #[serde(default)]
    pub(crate) session_id: Option<String>,
    #[serde(default)]
    pub(crate) model: Option<ModelInfo>,
    #[serde(default)]
    pub(crate) cost: Option<CostInfo>,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
    #[serde(default)]
    effort: Option<EffortInfo>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct ModelInfo {
    #[serde(default)]
    pub(crate) id: Option<String>,
    #[serde(default)]
    pub(crate) display_name: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub(crate) struct CostInfo {
    #[serde(default)]
    pub(crate) total_cost_usd: Option<f64>,
    #[serde(default)]
    total_duration_ms: Option<i64>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct RateLimits {
    #[serde(default)]
    five_hour: Option<RateLimitWindow>,
    #[serde(default)]
    seven_day: Option<RateLimitWindow>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct RateLimitWindow {
    #[serde(default)]
    used_percentage: Option<f64>,
    #[serde(default)]
    resets_at: Option<i64>, // seconds epoch
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct EffortInfo {
    #[serde(default)]
    level: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Payload of the `sensor-update` event to the frontend
// ─────────────────────────────────────────────────────────────────────────────

/// Official data derived from the statusLine JSON, ready to render.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SensorUpdate {
    provider: crate::providers::ProviderId,
    observed_at_ms: i64,
    five_hour_pct: Option<f64>,
    five_hour_resets_at: Option<i64>, // seconds epoch
    seven_day_pct: Option<f64>,
    seven_day_resets_at: Option<i64>,
    model_id: Option<String>,
    effort_level: Option<String>,
    cost_usd: Option<f64>,
    session_id: Option<String>,
}

impl SensorUpdate {
    fn from_input(i: &StatusInput) -> Self {
        let (five_hour_pct, five_hour_resets_at) = i
            .rate_limits
            .as_ref()
            .and_then(|r| r.five_hour.as_ref())
            .map(|w| (w.used_percentage, w.resets_at))
            .unwrap_or((None, None));
        let (seven_day_pct, seven_day_resets_at) = i
            .rate_limits
            .as_ref()
            .and_then(|r| r.seven_day.as_ref())
            .map(|w| (w.used_percentage, w.resets_at))
            .unwrap_or((None, None));
        SensorUpdate {
            provider: crate::providers::ProviderId::Claude,
            observed_at_ms: crate::providers::now_epoch_ms(),
            five_hour_pct,
            five_hour_resets_at,
            seven_day_pct,
            seven_day_resets_at,
            model_id: i.model.as_ref().and_then(|m| m.id.clone()),
            effort_level: i.effort.as_ref().and_then(|e| e.level.clone()),
            cost_usd: i.cost.as_ref().and_then(|c| c.total_cost_usd),
            session_id: i.session_id.clone(),
        }
    }
}

/// Payload of the `sensor-state` {connected} event to the frontend.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct StatePayload {
    provider: crate::providers::ProviderId,
    observed_at_ms: i64,
    connected: bool,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SensorSnapshot {
    update: Option<SensorUpdate>,
    state: StatePayload,
}

#[tauri::command]
pub fn sensor_snapshot() -> SensorSnapshot {
    let observed_at_ms = crate::providers::now_epoch_ms();
    let update = status_file()
        .and_then(|path| fs::read(path).ok())
        .and_then(|data| serde_json::from_slice::<StatusInput>(&data).ok())
        .map(|input| {
            let mut update = SensorUpdate::from_input(&input);
            update.observed_at_ms = observed_at_ms;
            update
        });
    SensorSnapshot {
        update,
        state: StatePayload {
            provider: crate::providers::ProviderId::Claude,
            observed_at_ms,
            connected: is_connected(SystemTime::now()),
        },
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Sensor file tail (dedicated thread)
// ─────────────────────────────────────────────────────────────────────────────

/// Starts the sensor file tail in a dedicated thread. Emits `sensor-update`
/// when the file changes and `sensor-state` {connected} based on its freshness.
/// Never panics; any failure is ignored (retried).
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        let mut last_mtime: Option<SystemTime> = None;
        let mut last_connected: Option<bool> = None;
        loop {
            let now = SystemTime::now();

            if let Some(input) = read_if_changed(&mut last_mtime) {
                let update = SensorUpdate::from_input(&input);
                // OFFICIAL data: % remaining for the tray ring = 100 - % used.
                // Priority over `engine`'s estimated writes is arbitrated by
                // `tray_icon::set_progress` itself, not here (D39).
                if let Some(used_pct) = update.five_hour_pct {
                    crate::tray_icon::set_progress(
                        &app,
                        crate::providers::ProviderId::Claude,
                        100.0 - used_pct,
                        crate::tray_icon::ProgressSource::Official,
                    );
                }
                let _ = app.emit("sensor-update", update);
            }

            // Connection state: the file exists and is fresh (< STALE_SECS).
            let connected = is_connected(now);
            if last_connected != Some(connected) {
                let _ = app.emit(
                    "sensor-state",
                    StatePayload {
                        provider: crate::providers::ProviderId::Claude,
                        observed_at_ms: crate::providers::now_epoch_ms(),
                        connected,
                    },
                );
                crate::providers::emit_health(
                    &app,
                    crate::providers::ProviderId::Claude,
                    crate::providers::ProviderComponent::Sensor,
                    if connected {
                        crate::providers::HealthStatus::Connected
                    } else {
                        crate::providers::HealthStatus::Degraded
                    },
                    None,
                );
                last_connected = Some(connected);
            }

            thread::sleep(Duration::from_millis(SENSOR_TAIL_INTERVAL_MS));
        }
    });
}

/// Reads and parses the sensor file only if its mtime advanced since the last read.
fn read_if_changed(last_mtime: &mut Option<SystemTime>) -> Option<StatusInput> {
    let path = status_file()?;
    let meta = fs::metadata(&path).ok()?;
    let mtime = meta.modified().ok()?;
    if Some(mtime) == *last_mtime {
        return None;
    }
    let data = fs::read(&path).ok()?;
    let input = serde_json::from_slice::<StatusInput>(&data).ok()?;
    *last_mtime = Some(mtime);
    Some(input)
}

/// `true` if the sensor file exists and was written less than `STALE_SECS` ago.
fn is_connected(now: SystemTime) -> bool {
    let Some(path) = status_file() else {
        return false;
    };
    let Ok(meta) = fs::metadata(&path) else {
        return false;
    };
    let Ok(mtime) = meta.modified() else {
        return false;
    };
    now.duration_since(mtime)
        .map(|d| d.as_secs() < STALE_SECS)
        .unwrap_or(false)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests — against the real statusLine JSON (official rate_limits, seconds).
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample with full rate_limits (Pro/Max subscriber), effort, and cost.
    const SAMPLE: &str = r#"{
      "session_id": "abc-123",
      "model": { "id": "claude-opus-4-8", "display_name": "Opus" },
      "cost": { "total_cost_usd": 0.01234, "total_duration_ms": 45000 },
      "rate_limits": {
        "five_hour": { "used_percentage": 23.5, "resets_at": 1738425600 },
        "seven_day": { "used_percentage": 41.2, "resets_at": 1738857600 }
      },
      "effort": { "level": "high" }
    }"#;

    #[test]
    fn parses_status_input_full() {
        let i: StatusInput = serde_json::from_str(SAMPLE).expect("must parse");
        let u = SensorUpdate::from_input(&i);
        assert_eq!(serde_json::to_value(&u).unwrap()["provider"], "claude");
        assert!(u.observed_at_ms > 0);
        assert_eq!(u.model_id.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(u.effort_level.as_deref(), Some("high"));
        assert!((u.five_hour_pct.unwrap() - 23.5).abs() < 1e-6);
        assert_eq!(u.five_hour_resets_at, Some(1_738_425_600)); // seconds, NOT ms
        assert!((u.seven_day_pct.unwrap() - 41.2).abs() < 1e-6);
        assert!((u.cost_usd.unwrap() - 0.01234).abs() < 1e-9);
    }

    #[test]
    fn sensor_state_uses_frontend_camel_case_contract() {
        let value = serde_json::to_value(StatePayload {
            provider: crate::providers::ProviderId::Claude,
            observed_at_ms: 42,
            connected: true,
        })
        .unwrap();
        assert_eq!(value["provider"], "claude");
        assert_eq!(value["observedAtMs"], 42);
        assert!(value.get("observed_at_ms").is_none());
    }

    /// Non Pro/Max subscriber → rate_limits absent. Must not break.
    #[test]
    fn tolerates_missing_rate_limits() {
        let json = r#"{ "model": { "id": "claude-sonnet-5" } }"#;
        let i: StatusInput = serde_json::from_str(json).expect("parses without rate_limits");
        let u = SensorUpdate::from_input(&i);
        assert_eq!(u.five_hour_pct, None);
        assert_eq!(u.seven_day_resets_at, None);
        assert_eq!(u.model_id.as_deref(), Some("claude-sonnet-5"));
    }

    /// Only five_hour, without seven_day (or vice versa).
    #[test]
    fn tolerates_partial_rate_limits() {
        let json = r#"{ "rate_limits": { "five_hour": { "used_percentage": 8 } } }"#;
        let i: StatusInput = serde_json::from_str(json).unwrap();
        let u = SensorUpdate::from_input(&i);
        assert!((u.five_hour_pct.unwrap() - 8.0).abs() < 1e-6);
        assert_eq!(u.five_hour_resets_at, None);
        assert_eq!(u.seven_day_pct, None);
    }

    #[test]
    fn empty_json_defaults() {
        let i: StatusInput = serde_json::from_str("{}").unwrap();
        let u = SensorUpdate::from_input(&i);
        assert_eq!(u.model_id, None);
        assert!(u.five_hour_pct.is_none());
    }

    /// `resets_at` arrives as a 10-digit integer (seconds) and is kept as
    /// i64 — pitfall A1: treating it as ms would give 1970-01-19.
    #[test]
    fn resets_at_kept_as_seconds() {
        let json = r#"{ "rate_limits": { "seven_day": { "used_percentage": 90, "resets_at": 1738857600 } } }"#;
        let i: StatusInput = serde_json::from_str(json).unwrap();
        let u = SensorUpdate::from_input(&i);
        let secs = u.seven_day_resets_at.unwrap();
        assert_eq!(secs.to_string().len(), 10, "epoch in seconds = 10 digits");
        assert!(secs > 1_700_000_000); // plausible for 2024+
    }
}
