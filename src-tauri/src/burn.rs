//! burn — sensor de `tok/s` **por respuesta** (D8).
//!
//! Sigue (tail) el JSONL de la sesión activa en `~/.claude/projects/**/*.jsonl`
//! y, al cerrarse cada turno, calcula `Δoutput / Δt_turno` y emite `burn-tick`.
//! Es el dato que ccusage no ofrece — pero **no es instantáneo**: el JSONL solo
//! estampa `usage` al terminar el mensaje (D8/DATA-ENGINE §Fuente 2), nunca
//! mid-generación (esto NO se puede acelerar poll-eando más rápido: el dato
//! sencillamente no existe en disco hasta ese instante). En turnos con
//! herramientas (varios mensajes `assistant` antes del cierre) sí se emite un
//! tick PARCIAL por cada mensaje intermedio, además del agregado final del
//! turno completo — feedback más temprano sin esperar al cierre (D27).
//!
//! Diseño sobrio (sin plugins, sin async framework, sin crates nuevas): un hilo
//! dedicado que hace `stat` + `read` del fichero cada 200 ms. No es el derroche
//! que D13 prohíbe (eso era spawn de Node por tick); un `stat` es una syscall
//! trivial. kqueue/inotify exigiría la crate `notify` — rechazada por el principio
//! W203 de mínimas piezas. El timestamp Zulu se parsea a mano (sin `chrono`):
//! el formato de Claude Code es siempre `YYYY-MM-DDTHH:MM:SS.mmmZ` (UTC, `Z`).

use std::collections::HashSet;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};

/// Cadencia del `stat` del JSONL (D13: evento-dirigido en espíritu; el `stat` no
/// es spawn de proceso — abrir+stat+leer-si-cambió un solo fichero cada 200 ms
/// es coste despreciable). Antes 1000 ms: se notaba como lag perceptible entre
/// el cierre real del turno y la aguja reaccionando; 200 ms lo baja a
/// imperceptible sin tocar la cadencia (5 s) de qué fichero es el activo.
const TAIL_INTERVAL_MS: u64 = 200;
/// Cada cuánto se rebusca el JSONL más reciente (la sesión activa puede rotar).
const ACTIVE_RESCAN_SECS: u64 = 5;

// ─────────────────────────────────────────────────────────────────────────────
// Timestamp Zulu → epoch-millis (sin `chrono`)
// Formato fijo de Claude Code: "2026-07-16T08:34:42.592Z"
// ─────────────────────────────────────────────────────────────────────────────

/// Convierte un timestamp Zulu a epoch-millis. `None` si el formato no casa.
fn parse_zulu_millis(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() != 24 || b[23] != b'Z' {
        return None;
    }
    let n = |start: usize| -> Option<i64> {
        std::str::from_utf8(&b[start..start + 2])
            .ok()?
            .parse::<i64>()
            .ok()
    };
    let y: i64 = std::str::from_utf8(&b[0..4]).ok()?.parse().ok()?;
    let mo = n(5)?;
    let d = n(8)?;
    let hh = n(11)?;
    let mi = n(14)?;
    let ss = n(17)?;
    let msec: i64 = std::str::from_utf8(&b[20..23]).ok()?.parse().ok()?;
    // Validación defensiva de rangos (Claude Code escribe valores válidos, pero un
    // campo fuera de rango produce epoch_ms silenciosamente incorrecto → None).
    if !(1..=12).contains(&mo)
        || !(1..=31).contains(&d)
        || !(0..=23).contains(&hh)
        || !(0..=59).contains(&mi)
        || !(0..=59).contains(&ss)
        || !(0..=999).contains(&msec)
    {
        return None;
    }
    let days = days_from_civil(y, mo as u64, d as u64);
    Some(
        days * 86_400_000
            + hh * 3_600_000
            + mi * 60_000
            + ss * 1000
            + msec,
    )
}

/// Días desde 1970-01-01 para una fecha civil (algoritmo de Howard Hinnant,
/// probado y libre de ramas). Proleto gregoriano.
fn days_from_civil(y: i64, m: u64, d: u64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as u64; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    era * 146_097 + doe as i64 - 719_468
}

// ─────────────────────────────────────────────────────────────────────────────
// Modelo serde de una línea del JSONL (solo lo que usamos)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct JsonlLine {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    message: Option<AssistantMsg>,
}

#[derive(Deserialize)]
struct AssistantMsg {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    output_tokens: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// Cálculo del turno — LÓGICA PURA (sin Tauri) → testeable
// ─────────────────────────────────────────────────────────────────────────────

/// Cierre de un turno: listo para emitir como `burn-tick`.
#[derive(Debug, Clone, PartialEq)]
struct BurnCalc {
    tok_per_s: f64,
    turn_output_tokens: u64,
    turn_duration_ms: i64,
    message_id: String,
    timestamp: String,
}

/// Estado del turno en curso. Acumula `output_tokens` (dedup por `message.id`)
/// desde el cierre anterior hasta el próximo `end_turn`/`stop_sequence`.
#[derive(Default)]
struct TurnState {
    /// `ts` del cierre del turno anterior (None al arrancar).
    last_end_ms: Option<i64>,
    /// Σ `output_tokens` deduplicados del turno en curso.
    turn_output: u64,
    /// `ts` del primer mensaje acumulado del turno (respaldo de Δt si no hay
    /// cierre anterior — p. ej. al enganchar un fichero a mitad de sesión).
    turn_start_ms: Option<i64>,
    /// `ts` del último mensaje visto (cualquiera, no solo cierres) — base del
    /// Δt de los ticks parciales intermedios. `None` al arrancar cada turno.
    last_msg_ms: Option<i64>,
    /// `message.id` ya contados (descarta reescrituras, que traen el mismo valor).
    seen: HashSet<String>,
}

impl TurnState {
    fn new() -> Self {
        Self::default()
    }

    /// Ingresa un mensaje assistant. Devuelve `Some(BurnCalc)` en dos casos:
    /// (a) este mensaje cierra el turno (`end_turn`/`stop_sequence`, agregado
    /// de TODO el turno) o (b) es un mensaje intermedio (`tool_use`, etc.) que
    /// no es el primero del turno — tick PARCIAL con solo sus propios
    /// tokens/Δt, para no esperar al cierre en turnos largos con herramientas.
    fn ingest(
        &mut self,
        msg_id: &str,
        out_tok: u64,
        ts_ms: i64,
        stop_reason: Option<&str>,
        timestamp: &str,
    ) -> Option<BurnCalc> {
        // Tokens contados SOLO la primera vez que vemos el id (las reescrituras
        // traen el mismo valor, D8). El cierre se procesa siempre: si un id visto
        // como `tool_use` reaparece como `end_turn`, el turno debe cerrar igual.
        let first_time = self.seen.insert(msg_id.to_string());
        if first_time {
            if self.turn_start_ms.is_none() {
                self.turn_start_ms = Some(ts_ms);
            }
            self.turn_output += out_tok;
        }

        let closes = matches!(stop_reason, Some("end_turn") | Some("stop_sequence"));

        if !closes {
            // Mensaje intermedio: tick parcial solo si es la primera vez que lo
            // vemos (una reescritura no es trabajo nuevo) y hay Δt real — el
            // primer mensaje del turno siempre tiene Δt=0 contra sí mismo, así
            // que no emite (correcto: aún no hay nada que medir).
            if !first_time || out_tok == 0 {
                return None;
            }
            let start_ms = self.last_msg_ms.or(self.turn_start_ms).unwrap_or(ts_ms);
            let dt_ms = ts_ms - start_ms;
            if dt_ms <= 0 {
                // ts no monótono: NO sellar last_msg_ms (igual que last_end_ms en
                // el cierre) para no perder la referencia del próximo Δt.
                return None;
            }
            self.last_msg_ms = Some(ts_ms);
            return Some(BurnCalc {
                tok_per_s: out_tok as f64 * 1000.0 / dt_ms as f64,
                turn_output_tokens: out_tok,
                turn_duration_ms: dt_ms,
                message_id: msg_id.to_string(),
                timestamp: timestamp.to_string(),
            });
        }

        // `turn_output == 0` descarta: turnos vacíos (0 tokens) y reescrituras de
        // un cierre ya emitido (que dejarían el acumulado a 0 tras el reset).
        if self.turn_output == 0 {
            return None;
        }

        // Δt = cierre actual − cierre anterior (o, en su defecto, inicio del turno).
        let start_ms = self.last_end_ms.or(self.turn_start_ms).unwrap_or(ts_ms);
        let dt_ms = ts_ms - start_ms;
        if dt_ms <= 0 {
            // ts no monótono (reescritura/reloj raro): no emitir NI sellar el cierre,
            // para no perder tokens acumulados ni falsear el próximo Δt.
            return None;
        }

        let calc = BurnCalc {
            tok_per_s: self.turn_output as f64 * 1000.0 / dt_ms as f64,
            turn_output_tokens: self.turn_output,
            turn_duration_ms: dt_ms,
            message_id: msg_id.to_string(),
            timestamp: timestamp.to_string(),
        };
        // Sellar el cierre SOLO al emitir: el turno siguiente empieza desde aquí.
        self.last_end_ms = Some(ts_ms);
        self.turn_output = 0;
        self.turn_start_ms = None;
        self.last_msg_ms = None;
        Some(calc)
    }
}

/// Procesa una línea cruda del JSONL. `Some(BurnCalc)` si cierra un turno.
fn process_line(state: &mut TurnState, line: &[u8]) -> Option<BurnCalc> {
    let parsed: JsonlLine = serde_json::from_slice(line).ok()?;
    if parsed.kind != "assistant" {
        return None;
    }
    let msg = parsed.message?;
    let usage = msg.usage?;
    let msg_id = msg.id?;
    let ts_ms = parse_zulu_millis(&parsed.timestamp)?;
    state.ingest(
        &msg_id,
        usage.output_tokens,
        ts_ms,
        msg.stop_reason.as_deref(),
        &parsed.timestamp,
    )
}

/// Acumula `chunk` en `leftover` y devuelve las líneas completas (sin `\n`),
/// dejando en `leftover` el resto sin `\n` para el próximo ciclo. Así un chunk
/// que corta una línea a medias se reensambla al llegar la siguiente tanda.
fn split_lines(leftover: &mut Vec<u8>, chunk: &[u8]) -> Vec<Vec<u8>> {
    leftover.extend_from_slice(chunk);
    let bytes = std::mem::take(leftover);
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            out.push(bytes[start..i].to_vec());
            start = i + 1;
        }
    }
    *leftover = bytes[start..].to_vec();
    out
}

// ─────────────────────────────────────────────────────────────────────────────
// Payload del evento al frontend
// ─────────────────────────────────────────────────────────────────────────────

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

// ─────────────────────────────────────────────────────────────────────────────
// Tail del JSONL activo
// ─────────────────────────────────────────────────────────────────────────────

/// Sigue un único fichero: posición de lectura (`pos`) + resto sin `\n`
/// (`leftover`) para no perder líneas escritas a medias. `pos` avanza por todos
/// los bytes leídos; el `leftover` se conserva sin releer (ver `drain`).
struct Tail {
    active: Option<PathBuf>,
    pos: u64,
    leftover: Vec<u8>,
    state: TurnState,
}

impl Tail {
    fn new() -> Self {
        Tail {
            active: None,
            pos: 0,
            leftover: Vec::new(),
            state: TurnState::new(),
        }
    }

    /// Re-selecciona el JSONL más reciente si la sesión activa rotó. Llamar con
    /// baja frecuencia (no cada tick): es un `readdir` sobre todos los proyectos.
    fn rescan(&mut self, projects_dir: &Path) {
        if let Some((latest, size)) = most_recent_jsonl(projects_dir) {
            if self.active.as_deref() != Some(latest.as_path()) {
                self.active = Some(latest);
                // EOF-start con el size de la MISMA metadata del rescan (evita una
                // segunda `stat` con race). Cero ruido histórico: la aguja arranca
                // en ralentí y reacciona solo a turnos que cierren desde ahora (D8).
                self.pos = size;
                self.leftover.clear();
                self.state = TurnState::new();
            }
        }
    }

    /// Drena los bytes nuevos del fichero activo y devuelve los turnos cerrados
    /// en este ciclo. NO emite — separa I/O de emisión para poder testear sin
    /// `AppHandle`. Actualiza `pos`/`leftover`/estado.
    fn drain(&mut self) -> Vec<BurnCalc> {
        let mut ticks = Vec::new();
        let Some(path) = self.active.clone() else {
            return ticks;
        };
        // Stat barato ANTES de abrir (D27 addendum): a 200 ms de cadencia, el
        // caso común (nada nuevo escrito) no debe pagar un `open()`+`fstat` —
        // un `metadata()` sin abrir el fichero basta para descartar el ciclo.
        let Ok(meta) = std::fs::metadata(&path) else {
            return ticks;
        };
        let len = meta.len();

        // Truncado detectado (el fichero encogió). Saltar al final — NUNCA a 0:
        // releer desde el inicio reemitiría burn-ticks históricos (ruido). Reset
        // de estado porque el contexto del fichero cambió.
        if len < self.pos {
            self.pos = len;
            self.leftover.clear();
            self.state = TurnState::new();
        }
        if len <= self.pos {
            return ticks;
        }

        let Ok(mut f) = OpenOptions::new().read(true).open(&path) else {
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

        // Parte por líneas; conserva el resto sin `\n` para el próximo ciclo.
        for line in split_lines(&mut self.leftover, &chunk[..n]) {
            if line.is_empty() {
                continue;
            }
            if let Some(calc) = process_line(&mut self.state, &line) {
                ticks.push(calc);
            }
        }
        // Avanzamos por TODOS los bytes leídos del fichero (no solo hasta el último
        // `\n`): el resto sin `\n` ya lo tenemos en `leftover`, no hace falta releerlo.
        // (Antes avanzábamos solo hasta el último `\n` → el leftover se releía y se
        // duplicaba, corrompiendo líneas parciales — ver test `drain_partial_line`.)
        self.pos += n as u64;
        ticks
    }

    /// Drena y emite `burn-tick` por cada turno cerrado.
    fn pump(&mut self, app: &AppHandle) {
        for calc in self.drain() {
            let _ = app.emit("burn-tick", BurnTick::from(calc));
        }
    }
}

/// Devuelve el `.jsonl` regular con mayor `mtime` bajo `projects_dir/**/*.jsonl`,
/// junto con su tamaño (de la misma `metadata`, sin una segunda `stat`). Recorre
/// a mano — sin `walkdir`. Ignora errores sueltos (nunca aborta). Exige fichero
/// regular (`is_file`) para no tragarse un directorio llamado `*.jsonl`.
fn most_recent_jsonl(projects_dir: &Path) -> Option<(PathBuf, u64)> {
    let dirs = fs_read_dir(projects_dir)?;
    let mut best: Option<(PathBuf, std::time::SystemTime, u64)> = None;
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
            if let Ok(mtime) = meta.modified() {
                if best.as_ref().is_none_or(|(_, t, _)| mtime > *t) {
                    best = Some((path, mtime, meta.len()));
                }
            }
        }
    }
    best.map(|(p, _, size)| (p, size))
}

fn fs_read_dir(p: &Path) -> Option<std::fs::ReadDir> {
    std::fs::read_dir(p).ok()
}

/// Arranca el sensor en un hilo dedicado. Busca `~/.claude/projects/` y tailea
/// el JSONL más reciente, emitiendo `burn-tick` por cada turno cerrado. Nunca
/// hace panic; cualquier fallo se ignora silenciosamente (se reintentará).
pub fn start(app: AppHandle) {
    thread::spawn(move || {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
            return;
        };
        let projects = home.join(".claude").join("projects");
        let mut tail = Tail::new();
        // Re-scan espaciado: el `readdir` sobre todos los proyectos no necesita
        // ir cada tick. Drain cada 1 s; re-scan cada N ticks.
        let scan_every = (ACTIVE_RESCAN_SECS * 1000 / TAIL_INTERVAL_MS).max(1);
        let mut tick = 0u64;

        loop {
            if tick.is_multiple_of(scan_every) {
                tail.rescan(&projects);
            }
            tail.pump(&app);
            tick = tick.wrapping_add(1);
            thread::sleep(Duration::from_millis(TAIL_INTERVAL_MS));
        }
    });
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests — contra casos controlados y contra el JSONL real del proyecto.
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zulu_epoch_origin() {
        assert_eq!(parse_zulu_millis("1970-01-01T00:00:00.000Z"), Some(0));
        assert_eq!(parse_zulu_millis("1970-01-01T00:00:01.000Z"), Some(1000));
        assert_eq!(parse_zulu_millis("1970-01-02T00:00:00.000Z"), Some(86_400_000));
    }

    #[test]
    fn zulu_real_delta_matches_d8() {
        // La diferencia entre el cierre anterior y el de 3008 tok (caso D8):
        // 1 min + 5 s + 278 ms = 65.278 s.
        let prev = parse_zulu_millis("2026-07-16T08:33:37.314Z").unwrap();
        let curr = parse_zulu_millis("2026-07-16T08:34:42.592Z").unwrap();
        assert_eq!(curr - prev, 65_278);
    }

    #[test]
    fn zulu_rejects_garbage() {
        assert_eq!(parse_zulu_millis("nope"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:34:42.592"), None); // sin Z
    }

    /// Línea assistant mínima válida para el parser.
    fn assistant(id: &str, ts: &str, out: u64, stop: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","stop_reason":"{stop}","usage":{{"output_tokens":{out}}}}}}}"#
        )
    }

    #[test]
    fn turn_calc_first_turn_uses_turn_start() {
        // Primer turno: sin cierre anterior, Δt = desde el primer mensaje.
        let mut s = TurnState::new();
        let a = assistant("m1", "2026-07-16T08:00:00.000Z", 200, "tool_use");
        let b = assistant("m2", "2026-07-16T08:00:10.000Z", 300, "end_turn");
        assert!(process_line(&mut s, a.as_bytes()).is_none()); // tool_use no cierra
        let calc = process_line(&mut s, b.as_bytes()).expect("cierra turno");
        // Δoutput=500, Δt=10 s → 50 tok/s
        assert_eq!(calc.turn_output_tokens, 500);
        assert_eq!(calc.turn_duration_ms, 10_000);
        assert!((calc.tok_per_s - 50.0).abs() < 1e-6, "tok/s = {}", calc.tok_per_s);
    }

    #[test]
    fn turn_calc_second_turn_uses_last_end() {
        let mut s = TurnState::new();
        process_line(&mut s, assistant("m1", "2026-07-16T08:00:00.000Z", 200, "tool_use").as_bytes());
        process_line(&mut s, assistant("m2", "2026-07-16T08:00:10.000Z", 300, "end_turn").as_bytes());
        // Segundo turno: cierre anterior = 08:00:10.
        process_line(&mut s, assistant("m3", "2026-07-16T08:00:15.000Z", 100, "tool_use").as_bytes());
        let calc = process_line(&mut s, assistant("m4", "2026-07-16T08:00:35.000Z", 400, "end_turn").as_bytes())
            .expect("cierra segundo turno");
        // Δoutput=500, Δt=08:00:35 − 08:00:10 = 25 s → 20 tok/s
        assert_eq!(calc.turn_output_tokens, 500);
        assert_eq!(calc.turn_duration_ms, 25_000);
        assert!((calc.tok_per_s - 20.0).abs() < 1e-6);
    }

    #[test]
    fn intermediate_tool_use_emits_partial_tick() {
        // Turno con 2 tool_use antes del cierre: el PRIMERO tiene Δt=0 (inicio
        // del turno, nada que medir aún) pero el SEGUNDO sí emite un tick
        // parcial con SOLO sus propios tokens/Δt (no acumulado), sin esperar
        // al cierre final — feedback temprano en turnos largos con
        // herramientas (D27).
        let mut s = TurnState::new();
        let a1 = assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use");
        let a2 = assistant("a2", "2026-07-16T08:00:05.000Z", 150, "tool_use");
        assert!(
            process_line(&mut s, a1.as_bytes()).is_none(),
            "primer mensaje del turno, Δt=0 contra sí mismo"
        );
        let partial = process_line(&mut s, a2.as_bytes())
            .expect("segundo mensaje intermedio emite tick parcial");
        // Δoutput = 150 (SOLO a2, no acumulado con a1), Δt = 5 s → 30 tok/s
        assert_eq!(partial.turn_output_tokens, 150);
        assert_eq!(partial.turn_duration_ms, 5_000);
        assert!((partial.tok_per_s - 30.0).abs() < 1e-6);

        // El cierre final SÍ agrega TODO el turno (100+150+200=450), no solo
        // lo que quedó desde el último tick parcial.
        let close = process_line(
            &mut s,
            assistant("a3", "2026-07-16T08:00:15.000Z", 200, "end_turn").as_bytes(),
        )
        .expect("cierra turno");
        assert_eq!(close.turn_output_tokens, 450);
        assert_eq!(close.turn_duration_ms, 15_000); // desde el inicio del turno
        assert!((close.tok_per_s - 30.0).abs() < 1e-6); // 450 / 15
    }

    #[test]
    fn intermediate_dt_non_monotonic_does_not_seal_last_msg() {
        // Espejo de `dt_non_monotonic_does_not_reset` pero para el tick
        // PARCIAL: un mensaje intermedio con ts no monótono no debe sellar
        // `last_msg_ms`, o el siguiente tick parcial calcularía su Δt contra
        // una referencia incorrecta (bug encontrado en code review, D27).
        let mut s = TurnState::new();
        let a1 = assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use");
        assert!(process_line(&mut s, a1.as_bytes()).is_none(), "Δt=0, inicio del turno");

        // a2 llega con ts ANTERIOR a a1 (reescritura/reloj raro) → Δt<0 → None,
        // y NO debe sellar last_msg_ms con este ts erróneo.
        let bad = assistant("a2", "2026-07-16T07:59:00.000Z", 50, "tool_use");
        assert!(process_line(&mut s, bad.as_bytes()).is_none(), "ts no monótono no emite");

        // a3 llega correctamente 5s después de a1 (NO de a2): si last_msg_ms se
        // hubiera sellado con el ts de a2, Δt sería absurdamente grande.
        let a3 = assistant("a3", "2026-07-16T08:00:05.000Z", 150, "tool_use");
        let partial = process_line(&mut s, a3.as_bytes())
            .expect("tick parcial usa la referencia correcta (a1, no a2)");
        assert_eq!(partial.turn_duration_ms, 5_000); // 08:00:05 − 08:00:00, no vs 07:59:00
        assert!((partial.tok_per_s - 30.0).abs() < 1e-6); // 150 tok / 5 s
    }

    #[test]
    fn dedup_by_message_id() {
        // Reescritura del mismo message.id → no se cuenta dos veces.
        let mut s = TurnState::new();
        let first = assistant("m1", "2026-07-16T08:00:00.000Z", 200, "tool_use");
        let rewrite = assistant("m1", "2026-07-16T08:00:01.000Z", 200, "tool_use");
        assert!(process_line(&mut s, first.as_bytes()).is_none());
        assert!(process_line(&mut s, rewrite.as_bytes()).is_none()); // ignorada
        let calc = process_line(&mut s, assistant("m2", "2026-07-16T08:00:10.000Z", 300, "end_turn").as_bytes())
            .unwrap();
        // 200 (una vez) + 300 = 500, no 700.
        assert_eq!(calc.turn_output_tokens, 500);
    }

    #[test]
    fn ignores_non_assistant_and_partial() {
        let mut s = TurnState::new();
        // user / system / basura → ninguna cierra.
        assert!(process_line(&mut s, br#"{"type":"user","timestamp":"2026-07-16T08:00:00.000Z"}"#).is_none());
        assert!(process_line(&mut s, b"esto no es json").is_none());
        assert!(process_line(&mut s, b"").is_none());
        // assistant sin usage → ignorado.
        assert!(process_line(&mut s, br#"{"type":"assistant","timestamp":"2026-07-16T08:00:00.000Z","message":{"id":"x"}}"#).is_none());
    }

    #[test]
    fn zulu_rejects_out_of_range() {
        // hora 24, min 60, etc. → None (no epoch silenciosamente incorrecto).
        assert_eq!(parse_zulu_millis("2026-07-16T24:00:00.000Z"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:60:00.000Z"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:00:60.000Z"), None);
        assert_eq!(parse_zulu_millis("2026-07-16T08:00:00.9999Z"), None); // formato
    }

    #[test]
    fn split_lines_handles_partial_writes() {
        let mut leftover = Vec::new();
        // chunk 1: "abc\ndef" → "def" queda como leftover (sin \n).
        let l1 = split_lines(&mut leftover, b"abc\ndef");
        assert_eq!(l1, vec![b"abc".to_vec()]);
        assert_eq!(leftover, b"def");
        // chunk 2: "\nghi" → completa "def" SIN duplicarlo.
        let l2 = split_lines(&mut leftover, b"\nghi");
        assert_eq!(l2, vec![b"def".to_vec()]);
        assert_eq!(leftover, b"ghi");
    }

    #[test]
    fn dedup_does_not_swallow_closure() {
        // BUG 2: un id visto como tool_use y reescrito como end_turn NO debe
        // ignorar el cierre. Los tokens se cuentan la primera vez (100).
        let mut s = TurnState::new();
        process_line(&mut s, assistant("mx", "2026-07-16T08:00:00.000Z", 100, "tool_use").as_bytes());
        let calc = process_line(
            &mut s,
            assistant("mx", "2026-07-16T08:00:10.000Z", 200, "end_turn").as_bytes(),
        )
        .expect("el cierre no debe tragarse");
        assert_eq!(calc.turn_output_tokens, 100); // contado una sola vez
    }

    #[test]
    fn dt_non_monotonic_does_not_reset() {
        // RIESGO 6: un cierre con ts no monótono no emite, NO actualiza
        // last_end_ms (no retrocede) y NO pierde los tokens acumulados.
        let mut s = TurnState::new();
        // Turno inicial VÁLIDO (con tool_use previo → dt>0) para fijar last_end_ms.
        process_line(&mut s, assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use").as_bytes());
        process_line(&mut s, assistant("a2", "2026-07-16T08:00:10.000Z", 50, "end_turn").as_bytes());
        // Turno en curso: tool_use acumula 300.
        process_line(&mut s, assistant("t1", "2026-07-16T08:00:15.000Z", 300, "tool_use").as_bytes());
        // cierre con ts ANTERIOR al último cierre válido (08:00:10) → dt < 0 → None.
        let bad = process_line(
            &mut s,
            assistant("c2", "2026-07-16T07:59:00.000Z", 0, "stop_sequence").as_bytes(),
        );
        assert!(bad.is_none(), "ts no monótono no emite");
        // cierre correcto posterior: Δt usa last_end_ms original (08:00:10), no 07:59:00.
        let good = process_line(
            &mut s,
            assistant("c3", "2026-07-16T08:00:30.000Z", 10, "end_turn").as_bytes(),
        )
        .expect("cierra con tokens conservados");
        assert_eq!(good.turn_duration_ms, 20_000); // 08:00:30 − 08:00:10, no 90 s
        assert_eq!(good.turn_output_tokens, 310); // 300 (t1) + 10 (c3); c2 aportó 0
    }

    /// BUG 1 (regresión del leftover duplicado): una línea escrita a medias en
    /// un ciclo debe completarse y procesarse UNA sola vez en el siguiente,
    /// sin corrupción ni replay. Usa un tmpfile real.
    #[test]
    fn drain_partial_line_not_duplicated() {
        use std::io::{Seek, Write};
        let path =
            std::env::temp_dir().join(format!("cc-autobahn-burn-test-{}.jsonl", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut tail = Tail::new();
        tail.active = Some(path.clone());
        tail.pos = 0;

        // Ciclo 1: tool_use completo + end_turn SIN '\n' final (escritura parcial).
        {
            let mut f = std::fs::File::create(&path).unwrap();
            writeln!(f, "{}", assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use")).unwrap();
            write!(f, "{}", assistant("a2", "2026-07-16T08:00:10.000Z", 200, "end_turn")).unwrap();
        }
        let t1 = tail.drain();
        assert!(t1.is_empty(), "sin '\\n' final → el turno aún no cierra");
        // La línea parcial (a2, sin '\n') queda retenida en leftover para el ciclo
        // siguiente; pos ya avanzó al final del fichero.
        assert!(tail.leftover.windows(2).any(|w| w == b"a2"),
            "el leftover retiene la línea parcial: {:?}",
            String::from_utf8_lossy(&tail.leftover));

        // Ciclo 2: se añade el '\n' que falta. La línea debe completarse sin duplicar.
        {
            use std::fs::OpenOptions;
            let mut f = OpenOptions::new().write(true).open(&path).unwrap();
            f.seek(SeekFrom::End(0)).unwrap();
            write!(f, "\n").unwrap();
        }
        let t2 = tail.drain();
        assert_eq!(t2.len(), 1, "cierra exactamente una vez");
        // Si el leftover se hubiera duplicado, la línea no parsearía → t2 vacío.
        assert_eq!(t2[0].turn_output_tokens, 300); // 100 + 200
        assert!((t2[0].tok_per_s - 30.0).abs() < 1e-6); // 300 / 10 s

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn end_to_end_against_real_jsonl() {
        // Procesa TODOS los `.jsonl` disponibles bajo `~/.claude/projects/`
        // (reset de TurnState por fichero, igual que el tail real al rotar
        // sesión) y verifica que el parser cierra turnos reales con tok/s > 0.
        // No depende de ningún proyecto concreto → portable y sin filtrar rutas.
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let Some(home) = home else { return }; // skip en máquinas sin HOME
        let projects = home.join(".claude/projects");
        let Some(files) = collect_jsonl(&projects) else {
            return; // sin logs → skip silencioso
        };

        let mut ticks: Vec<BurnCalc> = Vec::new();
        for path in &files {
            let Ok(data) = std::fs::read_to_string(path) else { continue };
            let mut s = TurnState::new(); // fresco por sesión, como el tail
            for line in data.lines() {
                if let Some(c) = process_line(&mut s, line.as_bytes()) {
                    ticks.push(c);
                }
            }
        }
        if ticks.is_empty() {
            return; // ninguna sesión con turnos cerrados → skip
        }
        // Todo tick emitido tiene tok/s finito y positivo.
        for c in &ticks {
            assert!(c.tok_per_s.is_finite() && c.tok_per_s > 0.0, "tok/s inválido");
            assert!(c.turn_output_tokens > 0, "turno sin output");
        }
    }

    /// Recoge todos los `.jsonl` regulares bajo `projects/*/*.jsonl`, ordenados.
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
}
