//! engine — motor de datos: detección de ccusage + polling de `blocks`.
//!
//! Todo el I/O vive aquí (nunca en la UI). No forkeamos ccusage: lo ejecutamos
//! como proceso hijo y parseamos su `--json` (ver docs/ARCHITECTURE.md, D1–D3).
//!
//! Diseño deliberadamente sobrio (sin plugins, sin async framework): un hilo
//! dedicado con `std::process::Command` + `std::thread::sleep`. Robusto,
//! serviciable, sin dependencias más allá de serde. El loop nunca hace panic;
//! cada fallo se transforma en un evento hacia el frontend.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Cadencia de `ccusage blocks` (D13: 10–30 s). El bloque de 5 h cambia lento;
/// pollear cada segundo sería derroche de spawn de proceso.
const POLL_INTERVAL_SECS: u64 = 15;

// ─────────────────────────────────────────────────────────────────────────────
// Detección del motor (cascada D9: global → npx → bunx → ninguno)
// ─────────────────────────────────────────────────────────────────────────────

/// Forma en que se invoca ccusage, resuelta una sola vez al arrancar.
#[derive(Debug, Clone, Copy)]
enum Engine {
    /// `ccusage` en el PATH (instalación global).
    Global,
    /// `npx -y ccusage@latest` (Node presente, sin instalar nada).
    Npx,
    /// `bunx ccusage` (Bun presente).
    Bunx,
}

impl Engine {
    /// Comando base; el llamante añade `blocks --active --json`.
    fn base_command(self) -> Command {
        match self {
            Engine::Global => Command::new("ccusage"),
            Engine::Npx => {
                let mut c = Command::new("npx");
                c.args(["-y", "ccusage@latest"]);
                c
            }
            Engine::Bunx => {
                let mut c = Command::new("bunx");
                c.arg("ccusage");
                c
            }
        }
    }

    /// Etiqueta corta para el evento `engine-detected`.
    fn label(self) -> &'static str {
        match self {
            Engine::Global => "ccusage",
            Engine::Npx => "npx",
            Engine::Bunx => "bunx",
        }
    }
}

/// Prueba el PATH una vez y devuelve el primer motor disponible (D9).
fn detect() -> Option<Engine> {
    if on_path("ccusage") {
        Some(Engine::Global)
    } else if on_path("npx") {
        Some(Engine::Npx)
    } else if on_path("bunx") {
        Some(Engine::Bunx)
    } else {
        None
    }
}

/// `true` si `bin` resuelve en el PATH. Recorre `$PATH` a mano — sin crate extra,
/// portable. En Windows contempla las extensiones ejecutables habituales.
fn on_path(bin: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    let exts: &[&str] = if cfg!(windows) {
        &["", ".cmd", ".exe", ".bat"]
    } else {
        &[""]
    };
    for dir in std::env::split_paths(&path) {
        for ext in exts {
            let candidate = dir.join(format!("{bin}{ext}"));
            if candidate.is_file() {
                return true;
            }
        }
    }
    false
}

/// `#[tauri::command]` ¿hay motor disponible AHORA MISMO? Para pintar la pantalla
/// "CHECK ENGINE" en el primer render sin depender de ganar la carrera contra el
/// evento `engine-missing` (el hilo de `start` puede emitirlo antes de que el
/// frontend termine de registrar el listener). Mismo patrón que `sensor_status`.
#[tauri::command]
pub fn engine_status() -> bool {
    detect().is_some()
}

/// `#[tauri::command]` Botón "INSTALAR MOTOR" (D9, Fase 4): lanza el instalador
/// oficial de Bun, actualiza el `PATH` del proceso ya arrancado (el instalador
/// solo lo añade al rc del shell, que este proceso no vuelve a leer) y reintenta
/// el motor. `Err` con mensaje legible para pintar en el overlay.
#[tauri::command]
pub fn install_bun(app: AppHandle) -> Result<String, String> {
    if let Some(engine) = detect() {
        start(app);
        return Ok(engine.label().to_string());
    }

    run_bun_installer()?;

    if let Some(dir) = bun_bin_dir() {
        prepend_path(&dir);
    }

    match detect() {
        Some(engine) => {
            start(app);
            Ok(engine.label().to_string())
        }
        None => Err("Bun se instaló pero bunx no aparece en PATH".to_string()),
    }
}

/// `~/.bun/bin`, destino fijo del instalador oficial.
#[cfg(unix)]
fn bun_bin_dir() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".bun").join("bin"))
}

#[cfg(not(unix))]
fn bun_bin_dir() -> Option<PathBuf> {
    None
}

/// Antepone `dir` al `PATH` del proceso actual (no del shell) para que `on_path`
/// y los `Command` siguientes encuentren `bunx` sin reiniciar la app.
fn prepend_path(dir: &Path) {
    let existing = std::env::var_os("PATH").unwrap_or_default();
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(&existing));
    if let Ok(joined) = std::env::join_paths(paths) {
        std::env::set_var("PATH", joined);
    }
}

/// Instalador oficial de Bun (https://bun.sh/install). macOS/Linux only por
/// ahora — el resto del proyecto tampoco está probado en Windows (D24).
#[cfg(unix)]
fn run_bun_installer() -> Result<(), String> {
    let status = Command::new("sh")
        .arg("-c")
        .arg("curl -fsSL https://bun.sh/install | bash")
        .status()
        .map_err(|e| format!("no se pudo lanzar el instalador de Bun: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("el instalador de Bun salió con {status}"))
    }
}

#[cfg(not(unix))]
fn run_bun_installer() -> Result<(), String> {
    Err("Instalación automática solo en macOS/Linux por ahora. Instala Bun a mano desde https://bun.sh y reinicia cc-autobahn.".to_string())
}

// ─────────────────────────────────────────────────────────────────────────────
// Modelo serde del JSON de `ccusage blocks --active --json`
// Estructurado contra la salida real (ccusage v20; capturada 2026-07-16).
// Campos opcionales/`default` porque los bloques de tipo "gap" omiten varios.
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
struct BlocksEnvelope {
    #[serde(default)]
    blocks: Vec<Block>,
}

/// Un bloque de facturación de 5 h. Reenviado tal cual al frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Block {
    id: String,
    #[serde(default)]
    is_active: bool,
    #[serde(default)]
    is_gap: bool,
    #[serde(default)]
    start_time: String,
    #[serde(default)]
    end_time: String,
    #[serde(default)]
    actual_end_time: Option<String>,
    #[serde(default)]
    cost_usd: f64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    entries: u64,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    token_counts: TokenCounts,
    #[serde(default)]
    burn_rate: Option<BurnRate>,
    #[serde(default)]
    projection: Option<Projection>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenCounts {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BurnRate {
    #[serde(default)]
    cost_per_hour: f64,
    #[serde(default)]
    tokens_per_minute: f64,
    /// Media suavizada de ccusage. NO es nuestro `tok/s` por respuesta (D8):
    /// ese lo calcula el tail de JSONL en Fase 2.
    #[serde(default)]
    tokens_per_minute_for_indicator: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Projection {
    #[serde(default)]
    remaining_minutes: u64,
    #[serde(default)]
    total_cost: f64,
    #[serde(default)]
    total_tokens: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Polling
// ─────────────────────────────────────────────────────────────────────────────

/// Ejecuta ccusage una vez y devuelve el bloque activo (si lo hay).
/// `Err` con mensaje legible ante cualquier fallo de spawn / exit / parseo.
fn poll_once(engine: Engine) -> Result<Option<Block>, String> {
    let output = engine
        .base_command()
        .args(["blocks", "--active", "--json"])
        .output()
        .map_err(|e| format!("no se pudo lanzar {}: {e}", engine.label()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ccusage salió con {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    let envelope: BlocksEnvelope = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("JSON de ccusage no parseable: {e}"))?;

    Ok(envelope.blocks.into_iter().find(|b| b.is_active && !b.is_gap))
}

/// Arranca el motor en un hilo dedicado. Detecta una vez; si no hay motor emite
/// `engine-missing` y termina. Si lo hay, poll en bucle emitiendo:
///   · `blocks-update`  → bloque activo (payload = Block)
///   · `blocks-idle`    → no hay bloque activo ahora mismo
///   · `engine-error`   → fallo puntual de este ciclo (payload = mensaje)
///   · `engine-detected`→ una vez, con la etiqueta del motor
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        let engine = match detect() {
            Some(e) => e,
            None => {
                let _ = app.emit("engine-missing", ());
                return;
            }
        };
        let _ = app.emit("engine-detected", engine.label());

        loop {
            match poll_once(engine) {
                Ok(Some(block)) => {
                    // % restante de la ventana de 5h para el anillo del tray —
                    // mismo criterio que applyEstimated() en main.js.
                    let pct_remaining = block
                        .projection
                        .as_ref()
                        .map(|p| p.remaining_minutes as f64 / crate::tray_icon::WINDOW_MIN * 100.0)
                        .unwrap_or(0.0);
                    crate::tray_icon::set_progress(&app, pct_remaining);
                    let _ = app.emit("blocks-update", &block);
                }
                Ok(None) => {
                    // Sin bloque activo: ventana sin gastar, anillo lleno.
                    crate::tray_icon::set_progress(&app, 100.0);
                    let _ = app.emit("blocks-idle", ());
                }
                Err(message) => {
                    let _ = app.emit("engine-error", message);
                }
            }
            thread::sleep(Duration::from_secs(POLL_INTERVAL_SECS));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Salida real de `ccusage v20 blocks --active --json` (capturada 2026-07-16).
    /// Bloquea el contrato del modelo serde contra el JSON verdadero.
    const REAL_SAMPLE: &str = r#"{
      "blocks": [{
        "actualEndTime": "2026-07-16T08:54:57.757Z",
        "burnRate": { "costPerHour": 16.81, "tokensPerMinute": 313145.96, "tokensPerMinuteForIndicator": 3093.26 },
        "costUSD": 24.846,
        "endTime": "2026-07-16T12:00:00.000Z",
        "entries": 262,
        "id": "2026-07-16T07:00:00.000Z",
        "isActive": true,
        "isGap": false,
        "models": ["claude-opus-4-8"],
        "projection": { "remainingMinutes": 185, "totalCost": 76.68, "totalTokens": 85701641 },
        "startTime": "2026-07-16T07:00:00.000Z",
        "tokenCounts": { "cacheCreationInputTokens": 544396, "cacheReadInputTokens": 26950933, "inputTokens": 46557, "outputTokens": 227752 },
        "totalTokens": 27769638
      }]
    }"#;

    #[test]
    fn parses_real_active_block() {
        let env: BlocksEnvelope = serde_json::from_str(REAL_SAMPLE).expect("debe parsear");
        let block = env
            .blocks
            .into_iter()
            .find(|b| b.is_active && !b.is_gap)
            .expect("hay bloque activo");
        assert_eq!(block.total_tokens, 27_769_638);
        assert_eq!(block.token_counts.output_tokens, 227_752);
        assert_eq!(block.projection.unwrap().remaining_minutes, 185);
        assert!(block.burn_rate.unwrap().cost_per_hour > 0.0);
    }

    /// Un bloque "gap" omite burnRate/projection: no debe romper el parseo.
    #[test]
    fn tolerates_gap_block_missing_fields() {
        let json = r#"{"blocks":[{"id":"x","isGap":true,"isActive":false}]}"#;
        let env: BlocksEnvelope = serde_json::from_str(json).expect("gap parsea");
        let b = &env.blocks[0];
        assert!(b.is_gap);
        assert!(b.burn_rate.is_none());
        assert!(b.projection.is_none());
    }

    /// JSON vacío / sin bloques: envelope válido, cero bloques.
    #[test]
    fn tolerates_empty() {
        let env: BlocksEnvelope = serde_json::from_str("{}").expect("vacío parsea");
        assert!(env.blocks.is_empty());
    }
}
