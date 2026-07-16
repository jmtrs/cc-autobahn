//! sensor — dato OFICIAL vía statusLine de Claude Code (D12).
//!
//! cc-autobahn es, además de la ventana, el comando `statusLine` que Claude Code
//! invoca pasándole el JSON de sesión por stdin — la ÚNICA fuente de `rate_limits`
//! oficial (ventana de 5 h / 7 d). El mismo binario funciona en dos modos:
//!   · `cc-autobahn statusline` → lee stdin, reemite la línea previa del usuario
//!     (chain, D-new-3) y vuelca el JSON a un fichero que la ventana tailea.
//!   · sin args → modo GUI (lo decide `main` antes de construir la webview).
//!
//! Diseño sobrio como `burn`/`engine`: cero crates nuevas, hilo dedicado con
//! `stat` + `read` cada 2 s (D13). Ojo: `resets_at` del JSON es epoch en SEGUNDOS,
//! no Zulu ms — se conserva crudo (NO reutiliza `burn::parse_zulu_millis`).

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Cadencia del `stat` del fichero sensor. No es spawn de proceso (D13).
const TAIL_INTERVAL_MS: u64 = 2000;
/// Si el fichero sensor lleva más de esto sin refresco → sensor "desconectado".
const STALE_SECS: u64 = 60;

// ─────────────────────────────────────────────────────────────────────────────
// Directorio de configuración de Claude Code (CLAUDE_CONFIG_DIR o ~/.claude)
// ─────────────────────────────────────────────────────────────────────────────

/// Resuelve `${CLAUDE_CONFIG_DIR:-$HOME/.claude}`. Única fuente de verdad: la usa
/// el modo statusline (escritura), el tail (lectura) y el install.
fn claude_config_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("CLAUDE_CONFIG_DIR") {
        return Some(PathBuf::from(dir));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from)?;
    Some(home.join(".claude"))
}

/// `~/.claude/cc-autobahn-status.json` — volcado por el modo statusline, taileado
/// por [`start`].
fn status_file() -> Option<PathBuf> {
    Some(claude_config_dir()?.join("cc-autobahn-status.json"))
}

/// `~/.claude/cc-autobahn/prev-statusline` — comando statusLine previo del usuario,
/// para el chain (D-new-3) y para el uninstall.
fn prev_statusline_file() -> Option<PathBuf> {
    Some(claude_config_dir()?.join("cc-autobahn").join("prev-statusline"))
}

// ─────────────────────────────────────────────────────────────────────────────
// Modelo serde del JSON de statusLine (snake_case, todo condicional)
// Estructurado contra la doc oficial + Wangnov/claude-code-statusline-pro.
// `resets_at` = epoch en SEGUNDOS (i64).
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
    resets_at: Option<i64>, // segundos epoch
}

#[derive(Debug, Default, Deserialize, Serialize)]
struct EffortInfo {
    #[serde(default)]
    level: Option<String>,
}

// ─────────────────────────────────────────────────────────────────────────────
// Payload del evento `sensor-update` al frontend
// ─────────────────────────────────────────────────────────────────────────────

/// Datos oficiales derivados del JSON de statusLine, listos para pintar.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SensorUpdate {
    five_hour_pct: Option<f64>,
    five_hour_resets_at: Option<i64>, // segundos epoch
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

/// Payload del evento `sensor-state` {connected} al frontend.
#[derive(Clone, Serialize)]
struct StatePayload {
    connected: bool,
}

// ─────────────────────────────────────────────────────────────────────────────
// Modo statusline — CLI: stdin → (chain previo a stdout) + fichero sensor
// ─────────────────────────────────────────────────────────────────────────────

/// Punto de entrada del modo `statusline` (`argv[1] == "statusline"`). Lee el
/// JSON de sesión de stdin, reemite el statusLine previo del usuario (chain,
/// D12/D-new-3) o una línea por defecto, y vuelca el JSON al fichero sensor.
/// Sale siempre con éxito (un statusline que falla ensucia el terminal).
pub fn run_statusline() {
    let mut buf = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut buf);

    if !chain_prev_statusline(&buf) {
        print_default_line(&buf);
    }
    write_status_file(&buf);
    let _ = std::io::stdout().flush();
}

/// Reejecuta el statusLine previo (guardado en `cc-autobahn/prev-statusline`) con
/// `buf` como stdin y reemite su stdout. `true` si emitió algo. macOS-first: usa
/// `/bin/sh`; en Windows el spawn falla y se cae a la línea por defecto.
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
    // El JSON de sesión cabe holgadamente en el pipe del kernel; el statusLine
    // previo lo lee o lo ignora. Si lo ignora, write_all termina igual.
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

/// Línea por defecto cuando no hay statusLine previo o el chain falló.
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

/// Escribe `buf` al fichero sensor con write tmp + rename atómico (mode 0600).
/// Descarta entradas que no sean JSON válido (no corromper el tail).
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
// Tail del fichero sensor (hilo dedicado)
// ─────────────────────────────────────────────────────────────────────────────

/// Arranca el tail del fichero sensor en un hilo dedicado. Emite `sensor-update`
/// cuando el fichero cambia y `sensor-state` {connected} según su frescura.
/// Nunca hace panic; cualquier fallo se ignora (se reintenta).
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        let mut last_mtime: Option<SystemTime> = None;
        let mut last_connected: Option<bool> = None;
        loop {
            let now = SystemTime::now();

            if let Some(input) = read_if_changed(&mut last_mtime) {
                let update = SensorUpdate::from_input(&input);
                // Dato OFICIAL: % restante para el anillo del tray = 100 - % gastado.
                if let Some(used_pct) = update.five_hour_pct {
                    crate::tray_icon::set_progress(&app, 100.0 - used_pct);
                }
                let _ = app.emit("sensor-update", update);
            }

            // Estado de conexión: el fichero existe y es fresco (< STALE_SECS).
            let connected = is_connected(now);
            if last_connected != Some(connected) {
                let _ = app.emit("sensor-state", StatePayload { connected });
                last_connected = Some(connected);
            }

            thread::sleep(Duration::from_millis(TAIL_INTERVAL_MS));
        }
    });
}

/// Lee y parsea el fichero sensor solo si su mtime avanzó desde la última lectura.
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

/// `true` si el fichero sensor existe y se escribió hace menos de `STALE_SECS`.
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
// Auto-instalación como statusLine (D12) — consent + backup + rollback.
//
// Muta `${cfg}/settings.json`, que es Zod-estricto en Claude Code: un campo mal
// deja al usuario sin config. Por eso el round-trip es con `serde_json::Value`
// (NUNCA struct tipado — no dropear campos desconocidos), con backup 0600 sin
// pisar, escritura tmp+rename atómica y re-validación post-escritura + rollback.
// El binario se COPIA a `${cfg}/cc-autobahn/cc-autobahn-statusline` (path estable)
// en vez de escribir `current_exe()`, que bajo translocación de Gatekeeper sería
// efímero (D-new-2).
// ─────────────────────────────────────────────────────────────────────────────

const STATUSLINE_BIN: &str = "cc-autobahn-statusline";
const BAK_SUFFIX: &str = ".cc-autobahn.bak";
const APP_KEY: &str = "cc-autobahn"; // settings["cc-autobahn"]
const PREV_KEY: &str = "prevStatusLine"; // settings["cc-autobahn"]["prevStatusLine"]

/// Estado de instalación informado al frontend.
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SensorStatus {
    installed: bool, // statusLine apunta a nuestro bin
    has_prev: bool, // hay un statusLine previo guardado (para rollback)
}

/// Vista previa de la instalación (para el modal de consentimiento).
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

/// Lee y parsea settings.json como `Value`. `None` si no existe o no parsea.
fn read_settings() -> Option<serde_json::Value> {
    let path = settings_path()?;
    let data = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

/// Path estable de la copia del binario (resuelve la translocación, D-new-2).
fn stable_bin_path(cfg: &Path) -> PathBuf {
    cfg.join("cc-autobahn").join(STATUSLINE_BIN)
}

/// El comando `statusLine` que escribiremos en settings.json.
fn statusline_command(cfg: &Path) -> String {
    format!("\"{}\" statusline", stable_bin_path(cfg).display())
}

/// `#[tauri::command]` ¿está instalado y apunta a nosotros?
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

/// `#[tauri::command]` Calcula la vista previa sin tocar nada (para confirmar).
#[tauri::command]
pub fn sensor_preview_install() -> Result<InstallPreview, String> {
    let cfg = claude_config_dir().ok_or("no se pudo resolver CLAUDE_CONFIG_DIR")?;
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

/// Transforma `settings` (Value) aplicando la instalación. Devuelve el
/// `statusLine` previo (para escribir `prev-statusline` del chain). PURA → testeable.
///
/// Idempotente: si el `statusLine` actual YA apunta a nosotros, NO nos capturamos
/// a nosotros mismos como `prev` (causaría un chain recursivo infinito en runtime).
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

/// Transforma `settings` (Value) deshaciendo la instalación. PURA → testeable.
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

/// `#[tauri::command]` Instala: backup → copia bin → reescribe settings → valida.
#[tauri::command]
pub fn install_sensor() -> Result<(), String> {
    let cfg = claude_config_dir().ok_or("no se pudo resolver CLAUDE_CONFIG_DIR")?;
    let settings_path = cfg.join("settings.json");
    let backup_path = cfg.join(format!("settings.json{BAK_SUFFIX}"));

    // 1. settings actuales ({} si no existe). Error si existe pero no parsea.
    let mut settings: serde_json::Value = if settings_path.exists() {
        let data = fs::read_to_string(&settings_path)
            .map_err(|e| format!("no se pudo leer settings.json: {e}"))?;
        serde_json::from_str(&data).map_err(|_| {
            "settings.json no es JSON estricto (¿tiene comentarios?). Configura el statusline a mano.".to_string()
        })?
    } else {
        serde_json::json!({})
    };

    // 2. backup 0600, SIN pisar uno preexistente (patrón caveman).
    if settings_path.exists() && !backup_path.exists() {
        copy_private(&settings_path, &backup_path)
            .map_err(|e| format!("backup falló: {e}"))?;
    }

    // 3. copiar el binario a path estable (D-new-2).
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let bin_dir = cfg.join("cc-autobahn");
    fs::create_dir_all(&bin_dir).map_err(|e| format!("create_dir: {e}"))?;
    let bin_path = stable_bin_path(&cfg);
    fs::copy(&exe, &bin_path).map_err(|e| format!("copy bin: {e}"))?;
    chmod_755(&bin_path);

    // 4. transformar settings (apply_install pura) y escribir el prev-statusline.
    let prev = apply_install(&mut settings, &statusline_command(&cfg));
    let prev_file = bin_dir.join("prev-statusline");
    match prev.as_ref().and_then(|v| v.get("command")).and_then(|c| c.as_str()) {
        Some(cmd) => {
            let _ = fs::write(&prev_file, cmd);
        }
        None => {
            let _ = fs::remove_file(&prev_file); // sin prev → chain usa default line
        }
    }

    // 5. escritura atómica (tmp+rename, 0600) + re-validación + rollback.
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
        Err("settings inválido tras escribir; se ha restaurado el backup".to_string())
    }
}

/// `#[tauri::command]` Desinstala: restaura el prevStatusLine (o lo elimina).
#[tauri::command]
pub fn uninstall_sensor() -> Result<(), String> {
    let cfg = claude_config_dir().ok_or("no se pudo resolver CLAUDE_CONFIG_DIR")?;
    let settings_path = cfg.join("settings.json");
    let Some(mut settings) = read_settings() else {
        return Ok(()); // nada que deshacer
    };
    apply_uninstall(&mut settings);
    write_settings_atomic(&settings_path, &settings.to_string())?;
    Ok(())
}

/// Escribe `bytes` en `path` vía tmp+rename atómico con modo 0600.
fn write_settings_atomic(path: &Path, bytes: &str) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    if !write_private(&tmp, bytes.as_bytes()) {
        return Err("no se pudo escribir settings.json".to_string());
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
// Tests — contra el JSON real de statusLine (rate_limits oficial, segundos).
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Sample con rate_limits completo (suscriptor Pro/Max), effort y coste.
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
        let i: StatusInput = serde_json::from_str(SAMPLE).expect("debe parsear");
        let u = SensorUpdate::from_input(&i);
        assert_eq!(u.model_id.as_deref(), Some("claude-opus-4-8"));
        assert_eq!(u.effort_level.as_deref(), Some("high"));
        assert!((u.five_hour_pct.unwrap() - 23.5).abs() < 1e-6);
        assert_eq!(u.five_hour_resets_at, Some(1_738_425_600)); // segundos, NO ms
        assert!((u.seven_day_pct.unwrap() - 41.2).abs() < 1e-6);
        assert!((u.cost_usd.unwrap() - 0.01234).abs() < 1e-9);
    }

    /// No suscriptor Pro/Max → rate_limits ausente. No debe romper.
    #[test]
    fn tolerates_missing_rate_limits() {
        let json = r#"{ "model": { "id": "claude-sonnet-5" } }"#;
        let i: StatusInput = serde_json::from_str(json).expect("parsea sin rate_limits");
        let u = SensorUpdate::from_input(&i);
        assert_eq!(u.five_hour_pct, None);
        assert_eq!(u.seven_day_resets_at, None);
        assert_eq!(u.model_id.as_deref(), Some("claude-sonnet-5"));
    }

    /// Solo five_hour, sin seven_day (o viceversa).
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

    /// `resets_at` llega como entero de 10 dígitos (segundos) y se conserva como
    /// i64 — trampa A1: tratarlo como ms sería 1970-01-19.
    #[test]
    fn resets_at_kept_as_seconds() {
        let json = r#"{ "rate_limits": { "seven_day": { "used_percentage": 90, "resets_at": 1738857600 } } }"#;
        let i: StatusInput = serde_json::from_str(json).unwrap();
        let u = SensorUpdate::from_input(&i);
        let secs = u.seven_day_resets_at.unwrap();
        assert_eq!(secs.to_string().len(), 10, "epoch en segundos = 10 dígitos");
        assert!(secs > 1_700_000_000); // plausible para 2024+
    }

    // ── Auto-instalación: transformación PURA de settings.json (D12) ──

    #[test]
    fn install_then_uninstall_roundtrip_with_caveman() {
        // settings con un statusLine previo (caveman) + un campo ajeno.
        let mut s: serde_json::Value = serde_json::json!({
            "statusLine": { "type": "command", "command": "bash /Users/x/caveman-statusline.sh" },
            "permissions": { "allow": ["ed:x"] }
        });
        let original = s.clone();

        apply_install(&mut s, "\"/p/cc-autobahn-statusline\" statusline");
        // statusLine ahora apunta a nuestro bin.
        assert!(s["statusLine"]["command"]
            .as_str()
            .unwrap()
            .contains("cc-autobahn-statusline"));
        // El previo queda guardado para el rollback/chain.
        assert_eq!(s["cc-autobahn"]["prevStatusLine"]["command"], original["statusLine"]["command"]);
        // Campo ajeno PRESERVADO (round-trip con Value, no struct tipado).
        assert_eq!(s["permissions"]["allow"][0], "ed:x");

        apply_uninstall(&mut s);
        // uninstall restaura el statusLine original y elimina nuestra clave.
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
        // Sin statusLine previo → prevStatusLine es null.
        assert!(s["cc-autobahn"]["prevStatusLine"].is_null());
        apply_uninstall(&mut s);
        // No había prev → uninstall elimina statusLine (no deja basura).
        assert!(s.get("statusLine").is_none());
        assert!(s.get("cc-autobahn").is_none());
    }

    #[test]
    fn reinstall_keeps_original_prev_no_loop() {
        // Ya instalado con un prev real (caveman). Reinstalar NO debe capturarse a
        // sí mismo como prev → evitaría un chain recursivo infinito en runtime.
        let mut s = serde_json::json!({
            "statusLine": { "type": "command", "command": "\"/p/cc-autobahn-statusline\" statusline" },
            "cc-autobahn": { "prevStatusLine": { "type": "command", "command": "bash prev.sh" } }
        });
        apply_install(&mut s, "\"/p/cc-autobahn-statusline\" statusline");
        // El prev conservado sigue siendo el original, NO nuestro propio command.
        assert_eq!(
            s["cc-autobahn"]["prevStatusLine"]["command"],
            serde_json::json!("bash prev.sh")
        );
    }
}
