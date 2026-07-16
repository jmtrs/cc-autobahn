//! sensor — OFFICIAL data via Claude Code's statusLine (D12).
//!
//! cc-autobahn is, in addition to the window, the `statusLine` command that Claude Code
//! invokes, passing it the session JSON via stdin — the ONLY source of official
//! `rate_limits` (5h / 7d window). The same binary works in two modes:
//!   · `cc-autobahn statusline` → reads stdin, re-emits the user's previous line
//!     (chain, D-new-3) and dumps the JSON to a file that the window tails.
//!   · no args → GUI mode (decided by `main` before building the webview).
//!
//! Sober design like `burn`/`engine`: zero new crates, dedicated thread with
//! `stat` + `read` every 2s (D13). Careful: `resets_at` in the JSON is epoch in SECONDS,
//! not Zulu ms — kept raw (does NOT reuse `burn::parse_zulu_millis`).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Cadence of the sensor file `stat`. Not a process spawn (D13).
const TAIL_INTERVAL_MS: u64 = 2000;
/// If the sensor file hasn't refreshed in longer than this → sensor "disconnected".
const STALE_SECS: u64 = 60;

// ─────────────────────────────────────────────────────────────────────────────
// Claude Code config directory (CLAUDE_CONFIG_DIR or ~/.claude)
// ─────────────────────────────────────────────────────────────────────────────

/// Resolves `${CLAUDE_CONFIG_DIR:-$HOME/.claude}`. Single source of truth: used
/// by statusline mode (write), the tail (read), and install.
fn claude_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".claude"))
}

/// `~/.claude/cc-autobahn-status.json` — dumped by statusline mode, tailed
/// by [`start`].
fn status_file() -> Option<PathBuf> {
    Some(claude_config_dir()?.join("cc-autobahn-status.json"))
}

/// `~/.claude/cc-autobahn/prev-statusline` — the user's previous statusLine command,
/// for the chain (D-new-3) and for uninstall.
fn prev_statusline_file() -> Option<PathBuf> {
    Some(claude_config_dir()?.join("cc-autobahn").join("prev-statusline"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Serde model of the statusLine JSON (snake_case, everything optional)
// Structured against the official docs + Wangnov/claude-code-statusline-pro.
// `resets_at` = epoch in SECONDS (i64).
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Deserialize, Serialize)]
struct StatusInput {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    model: Option<ModelInfo>,
    #[serde(default)]
    cost: Option<CostInfo>,
    #[serde(default)]
    rate_limits: Option<RateLimits>,
    #[serde(default)]
    effort: Option<EffortInfo>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct ModelInfo {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    display_name: Option<String>,
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct CostInfo {
    #[serde(default)]
    total_cost_usd: Option<f64>,
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
struct StatePayload {
    connected: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// statusline mode — CLI: stdin → (previous chain to stdout) + sensor file
// ─────────────────────────────────────────────────────────────────────────────

/// Entry point of `statusline` mode (`argv[1] == "statusline"`). Reads the
/// session JSON from stdin, re-emits the user's previous statusLine (chain,
/// D12/D-new-3) or a default line, and dumps the JSON to the sensor file.
/// Always exits successfully (a failing statusline messes up the terminal).
pub fn run_statusline() {
    let mut buf = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut buf);

    if !chain_prev_statusline(&buf) {
        print_default_line(&buf);
    }
    write_status_file(&buf);
    let _ = std::io::stdout().flush();
}

/// Re-runs the previous statusLine (saved in `cc-autobahn/prev-statusline`) with
/// `buf` as stdin and re-emits its stdout. `true` if it emitted something. macOS-first: uses
/// `/bin/sh`; on Windows the spawn fails and it falls back to the default line.
fn chain_prev_statusline(buf: &[u8]) -> bool {
    let Some(cmd_path) = prev_statusline_file() else {
        return false;
    };
    let Ok(cmd) = fs::read_to_string(&cmd_path) else {
        return false;
    };
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return false;
    }
    let Ok(mut child) = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    // The session JSON comfortably fits the kernel pipe; the previous statusLine
    // either reads it or ignores it. If it ignores it, write_all still finishes.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(buf);
    }
    let Ok(output) = child.wait_with_output() else {
        return false;
    };
    if output.stdout.is_empty() {
        return false;
    }
    let _ = std::io::stdout().write_all(&output.stdout);
    true
}

/// Default line when there's no previous statusLine or the chain failed.
fn print_default_line(buf: &[u8]) {
    let parsed: StatusInput = serde_json::from_slice(buf).unwrap_or_default();
    let model = parsed
        .model
        .as_ref()
        .and_then(|m| m.display_name.clone().or_else(|| m.id.clone()))
        .unwrap_or_else(|| "claude".to_string());
    let cost = parsed
        .cost
        .as_ref()
        .and_then(|c| c.total_cost_usd)
        .map(|v| format!(" · ${v:.2}"))
        .unwrap_or_default();
    println!("cc-autobahn · {model}{cost}");
}

/// Writes `buf` to the sensor file via tmp write + atomic rename (mode 0600).
/// Discards entries that aren't valid JSON (avoid corrupting the tail).
fn write_status_file(buf: &[u8]) {
    let Some(path) = status_file() else {
        return;
    };
    let Some(dir) = path.parent() else {
        return;
    };
    if serde_json::from_slice::<serde_json::Value>(buf).is_err() {
        return;
    }
    let _ = fs::create_dir_all(dir);
    let tmp = path.with_extension("json.tmp");
    if write_private(&tmp, buf) {
        let _ = fs::rename(&tmp, &path);
    }
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, buf: &[u8]) -> bool {
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
fn write_private(path: &std::path::Path, buf: &[u8]) -> bool {
    fs::write(path, buf).is_ok()
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
                if let Some(used_pct) = update.five_hour_pct {
                    crate::tray_icon::set_progress(&app, 100.0 - used_pct);
                }
                let _ = app.emit("sensor-update", update);
            }

            // Connection state: the file exists and is fresh (< STALE_SECS).
            let connected = is_connected(now);
            if last_connected != Some(connected) {
                let _ = app.emit("sensor-state", StatePayload { connected });
                last_connected = Some(connected);
            }

            thread::sleep(Duration::from_millis(TAIL_INTERVAL_MS));
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
// Auto-install as statusLine (D12) — consent + backup + rollback.
//
// Mutates `${cfg}/settings.json`, which is Zod-strict in Claude Code: one bad
// field leaves the user without config. That's why the round-trip uses
// `serde_json::Value` (NEVER a typed struct — don't drop unknown fields), with
// a non-overwriting 0600 backup, atomic tmp+rename write, and post-write
// re-validation + rollback. The binary is COPIED to
// `${cfg}/cc-autobahn/cc-autobahn-statusline` (stable path) instead of writing
// `current_exe()`, which would be ephemeral under Gatekeeper translocation
// (D-new-2).
// ─────────────────────────────────────────────────────────────────────────────

const STATUSLINE_BIN: &str = "cc-autobahn-statusline";
const BAK_SUFFIX: &str = ".cc-autobahn.bak";
const APP_KEY: &str = "cc-autobahn"; // settings["cc-autobahn"]
const PREV_KEY: &str = "prevStatusLine"; // settings["cc-autobahn"]["prevStatusLine"]

/// Installation state reported to the frontend.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SensorStatus {
    installed: bool, // statusLine points to our binary
    has_prev: bool, // there's a previous statusLine saved (for rollback)
}

/// Installation preview (for the consent modal).
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallPreview {
    prev_status_line: Option<serde_json::Value>,
    new_command: String,
    backup_path: String,
}

fn settings_path() -> Option<PathBuf> {
    Some(claude_config_dir()?.join("settings.json"))
}

/// Reads and parses settings.json as a `Value`. `None` if it doesn't exist or fails to parse.
fn read_settings() -> Option<serde_json::Value> {
    let path = settings_path()?;
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
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
    let Some(v) = read_settings() else {
        return SensorStatus { installed: false, has_prev: false };
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
    SensorStatus { installed, has_prev }
}

/// `#[tauri::command]` Computes the preview without touching anything (for confirmation).
#[tauri::command]
pub fn sensor_preview_install() -> Result<InstallPreview, String> {
    let cfg = claude_config_dir().ok_or("could not resolve CLAUDE_CONFIG_DIR")?;
    let prev = read_settings()
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
fn apply_install(
    settings: &mut serde_json::Value,
    command: &str,
) -> Option<serde_json::Value> {
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

    // 1. Current settings ({} if it doesn't exist). Error if it exists but fails to parse.
    let mut settings: serde_json::Value = if settings_path.exists() {
        let data = fs::read_to_string(&settings_path)
            .map_err(|e| format!("could not read settings.json: {e}"))?;
        serde_json::from_str(&data).map_err(|_| {
            "settings.json is not strict JSON (does it have comments?). Configure the statusline manually.".to_string()
        })?
    } else {
        serde_json::json!({})
    };

    // 2. 0600 backup, WITHOUT overwriting a pre-existing one (caveman pattern).
    if settings_path.exists() && !backup_path.exists() {
        copy_private(&settings_path, &backup_path)
            .map_err(|e| format!("backup failed: {e}"))?;
    }

    // 3. copy the binary to a stable path (D-new-2).
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let bin_dir = cfg.join("cc-autobahn");
    fs::create_dir_all(&bin_dir).map_err(|e| format!("create_dir: {e}"))?;
    let bin_path = stable_bin_path(&cfg);
    fs::copy(&exe, &bin_path).map_err(|e| format!("copy bin: {e}"))?;
    chmod_755(&bin_path);

    // 4. transform settings (pure apply_install) and write the prev-statusline.
    let prev = apply_install(&mut settings, &statusline_command(&cfg));
    let prev_file = bin_dir.join("prev-statusline");
    match prev.as_ref().and_then(|v| v.get("command")).and_then(|c| c.as_str()) {
        Some(cmd) => {
            let _ = fs::write(&prev_file, cmd);
        }
        None => {
            let _ = fs::remove_file(&prev_file); // no prev → chain uses default line
        }
    }

    // 5. atomic write (tmp+rename, 0600) + re-validation + rollback.
    write_settings_atomic(&settings_path, &settings.to_string())?;
    let valid = fs::read_to_string(&settings_path)
        .ok()
        .and_then(|d| serde_json::from_str::<serde_json::Value>(&d).ok())
        .is_some();
    if valid {
        Ok(())
    } else {
        if backup_path.exists() {
            let _ = fs::rename(&backup_path, &settings_path);
        }
        Err("settings invalid after writing; backup has been restored".to_string())
    }
}

/// `#[tauri::command]` Uninstalls: restores prevStatusLine (or removes it).
#[tauri::command]
pub fn uninstall_sensor() -> Result<(), String> {
    let cfg = claude_config_dir().ok_or("could not resolve CLAUDE_CONFIG_DIR")?;
    let settings_path = cfg.join("settings.json");
    let Some(mut settings) = read_settings() else {
        return Ok(()); // nothing to undo
    };
    apply_uninstall(&mut settings);
    write_settings_atomic(&settings_path, &settings.to_string())?;
    Ok(())
}

/// Writes `bytes` to `path` via atomic tmp+rename with mode 0600.
fn write_settings_atomic(path: &Path, bytes: &str) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    if !write_private(&tmp, bytes.as_bytes()) {
        return Err("could not write settings.json".to_string());
    }
    fs::rename(&tmp, path).map_err(|e| format!("rename settings: {e}"))
}

#[cfg(unix)]
fn chmod_755(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn chmod_755(_path: &Path) {}

#[cfg(unix)]
fn copy_private(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::copy(src, dst)?;
    let mut perms = fs::metadata(dst)?.permissions();
    perms.set_mode(0o600);
    fs::set_permissions(dst, perms)?;
    Ok(())
}

#[cfg(not(unix))]
fn copy_private(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::copy(src, dst)?;
    Ok(())
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
        assert_eq!(u.model_id.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(u.effort_level.as_deref(), Some("high"));
        assert!((u.five_hour_pct.unwrap() - 23.5).abs() < 1e-6);
        assert_eq!(u.five_hour_resets_at, Some(1_738_425_600)); // seconds, NOT ms
        assert!((u.seven_day_pct.unwrap() - 41.2).abs() < 1e-6);
        assert!((u.cost_usd.unwrap() - 0.01234).abs() < 1e-9);
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

    // ── Auto-install: PURE transformation of settings.json (D12) ──

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
        assert_eq!(s["cc-autobahn"]["prevStatusLine"]["command"], original["statusLine"]["command"]);
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
