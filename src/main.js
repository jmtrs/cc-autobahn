// cc-autobahn — frontend shell.
// Pinta el cluster W203 y escucha los sensores del backend (engine.rs + burn.rs).
// Reloj + barra de segmentos en estático; velocímetro = tok/s por respuesta con
// muelle físico (D8): salta al completar turno y decae a ralentí. NO es
// instantáneo — el JSONL solo reporta usage al cerrar el turno (ver D8/D11).

const SEGMENT_COUNT = 12;

/** Build the autonomie segment bar (fuel-gauge style). */
function buildSegments(filled) {
  const bar = document.getElementById("segments");
  bar.innerHTML = "";
  for (let i = 0; i < SEGMENT_COUNT; i++) {
    const seg = document.createElement("div");
    seg.className = i < filled ? "seg on" : "seg";
    bar.appendChild(seg);
  }
}

/** Tick the trip-computer clock, like the W203 bottom-right time. */
function tickClock() {
  const el = document.getElementById("clock");
  const now = new Date();
  const hh = String(now.getHours()).padStart(2, "0");
  const mm = String(now.getMinutes()).padStart(2, "0");
  el.textContent = `${hh}:${mm}`;
}

// ─────────────────────────────────────────────────────────────────────────────
// Velocímetro — muelle físico (D8).
// La aguja salta al tok/s del turno completado y decae con muelle a ralentí.
// Es la lectura HONESTA: "tok/s por respuesta", nunca "instantáneo" (D11).
// ─────────────────────────────────────────────────────────────────────────────

const burn = {
  target: 0, // tok/s objetivo (último tick, o decay en ralentí)
  pos: 0, // valor mostrado (animado por el spring)
  vel: 0, // velocidad del spring → overshoot mecánico
  lastTickAt: 0, // performance.now() del último burn-tick
  SPRING_K: 0.2, // rigidez del muelle
  SPRING_DAMP: 0.75, // amortiguación (>0 = underdamped, overshoot)
  IDLE_AFTER_MS: 2000, // sin tick fresco → ralentí
  IDLE_DECAY: 0.95, // por frame, el target decae hacia 0
};

/** Formatea tok/s al estilo VFD: "7.2", "55", "1.5k". */
function formatTps(tps) {
  if (tps < 0.5) return "0";
  if (tps < 10) return tps.toFixed(1);
  if (tps < 1000) return Math.round(tps).toString();
  return (tps / 1000).toFixed(1) + "k";
}

/** Bucle de animación del muelle. Corre siempre (también para decaer). */
function burnFrame(now) {
  // Ralentí: sin tick fresco, el target decae hacia 0.
  if (burn.lastTickAt && now - burn.lastTickAt > burn.IDLE_AFTER_MS) {
    burn.target *= burn.IDLE_DECAY;
    if (burn.target < 0.5) burn.target = 0;
  }
  // Integración del spring: force → vel (amortiguada) → posición.
  const force = (burn.target - burn.pos) * burn.SPRING_K;
  burn.vel = (burn.vel + force) * burn.SPRING_DAMP;
  burn.pos += burn.vel;
  if (burn.pos < 0) burn.pos = 0;

  document.getElementById("burn").textContent = formatTps(burn.pos);
  requestAnimationFrame(burnFrame);
}

/** Maneja un burn-tick del backend (turno cerrado). */
function onBurnTick(payload) {
  // payload = { tokPerS, turnOutputTokens, turnDurationMs, messageId, timestamp }
  const tps = Number(payload?.tokPerS) || 0;
  burn.target = tps;
  burn.lastTickAt = performance.now();
  // Lectura secundaria en el footer: tok/s crudo del último turno (sin muelle).
  document.getElementById("burn-inst").textContent = `${Math.round(tps)} tok/s`;
}

/**
 * Wire the backend engine events (see src-tauri/src/engine.rs + burn.rs).
 * Guarded: under plain `vite` (no Tauri) there is no IPC, so we skip silently.
 * blocks-update still only logs; that wiring (coste/proyección) lands in Phase 3.
 */
async function wireEngine() {
  if (!("__TAURI_INTERNALS__" in window)) return; // running outside Tauri
  const { listen } = await import("@tauri-apps/api/event");

  listen("engine-detected", (e) => console.info("[engine] motor:", e.payload));
  listen("engine-missing", () => console.warn("[engine] sin motor (CHECK ENGINE)"));
  listen("engine-error", (e) => console.error("[engine] error:", e.payload));
  listen("blocks-idle", () => console.info("[engine] sin bloque activo"));
  listen("blocks-update", (e) => console.info("[engine] blocks-update:", e.payload));
  listen("burn-tick", (e) => {
    console.info("[burn] tok/s por respuesta:", e.payload);
    onBurnTick(e.payload);
  });
}

function init() {
  // Placeholder autonomie: ~62% of the 5h window remaining.
  buildSegments(Math.round(SEGMENT_COUNT * 0.62));
  tickClock();
  setInterval(tickClock, 1000);
  wireEngine();
  requestAnimationFrame(burnFrame); // arranca en ralentí (pos=0), fiel al coche
}

window.addEventListener("DOMContentLoaded", init);
