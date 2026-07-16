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
  // Sigue contando aunque el sensor esté momentáneamente callado — el reset
  // conocido (fiveHourResetsAtMs) sigue siendo válido igual (ver D-review).
  refreshAutonomie();
  renderFooterMetric(); // poda los buffers PACE/AUTO aunque no llegue evento nuevo
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

/** Maneja un burn-tick del backend (turno cerrado o mensaje intermedio, D27). */
function onBurnTick(payload) {
  // payload = { tokPerS, turnOutputTokens, turnDurationMs, messageId, timestamp }
  const tps = Number(payload?.tokPerS) || 0;
  burn.target = tps;
  burn.lastTickAt = performance.now();
  // Buffer deslizante para la métrica PACE del footer (ver renderFooterMetric).
  const tokens = Number(payload?.turnOutputTokens) || 0;
  if (tokens > 0) {
    recentTicks.push({ recvAt: Date.now(), tokens });
  }
  renderFooterMetric();
}

// ─────────────────────────────────────────────────────────────────────────────
// Trip-computer readouts — datos del bloque activo de ccusage (blocks-update).
// Cableado Fase 3 Pista A. La autonomía aquí es ESTIMADA (proyección de tokens
// de ccusage): el sensor statusline (Pista B) la sobreescribe con la oficial
// rate_limits.five_hour cuando llega. Marca "EST" obligatoria mientras tanto.
// ─────────────────────────────────────────────────────────────────────────────

const WINDOW_MIN = 300; // ventana de facturación de 5 h, en minutos

/** Formatea tokens al estilo VFD: "999", "1.5k", "850k", "1.24M", "2.1G". */
function formatTokens(n) {
  if (!(n >= 1)) return "0";
  if (n < 1e3) return String(Math.round(n));
  if (n < 1e6) return (n / 1e3).toFixed(n < 1e5 ? 1 : 0).replace(/\.0$/, "") + "k";
  if (n < 1e9) return (n / 1e6).toFixed(2) + "M";
  return (n / 1e9).toFixed(2) + "G";
}

/** Minutos restantes → "3h12" (autonomía de la ventana). */
function formatHMin(minutes) {
  if (!(minutes >= 0)) return "—";
  // Redondear UNA vez a minuto entero antes de partir en h/m: redondear cada
  // parte por separado puede dar m=60 (ej. 119.5 → h=1, round(59.5)=60 →
  // "1h60" en vez de "2h00"; bug encontrado en revisión de fórmulas).
  const total = Math.round(minutes);
  const h = Math.floor(total / 60);
  const m = total % 60;
  return `${h}h${String(m).padStart(2, "0")}`;
}

/** ms de duración → "H:MM" (tiempo desde el inicio del bloque). */
function formatDurationMs(ms) {
  if (!(ms > 0)) return "0:00";
  const totalMin = Math.floor(ms / 60000);
  return `${Math.floor(totalMin / 60)}:${String(totalMin % 60).padStart(2, "0")}`;
}

// ─────────────────────────────────────────────────────────────────────────────
// Estado del sensor oficial. Cuando el statusLine está conectado, sus datos
// (rate_limits, model.id, effort) tienen PRIORIDAD sobre la proyección del bloque
// (D11: lo oficial nunca se estima). Al desconectarse, se vuelve a la proyección
// con marca "EST".
// ─────────────────────────────────────────────────────────────────────────────

let lastBlock = null; // último blocks-update, para re-aplicar estimación al desconectar
// `sensorConnected` = ¿está el sensor pusheando FRESCO ahora mismo? (D-review:
// una pausa normal sin renderizado de Claude Code lo pone en false unos
// segundos, sin ser un "nunca conectado" real). `everSensorConnected` es
// pegajoso — una vez true, ya no se vuelve a caer a la proyección de ccusage
// (sistema de ventana de 5h independiente, el salto entre ambos se veía
// absurdo: "0h17" oficial → "EST 4h31" de ccusage).
let sensorConnected = false; // ¿llega dato oficial del statusLine?
let everSensorConnected = false; // ¿llegó alguna vez? (pegajoso, ver arriba)
let fiveHourResetsAtMs = 0; // epoch-ms del reset de 5h (cuenta atrás, refresca el reloj)

// ─────────────────────────────────────────────────────────────────────────────
// Footer: PACE (ritmo reciente vs. medio del bloque) / AUTO (autonomía
// ajustada al ritmo reciente, solo sensor oficial). Sustituye al antiguo
// "ÚLT tok/s" (D27 lo volvió ambiguo: turno completo vs. mensaje intermedio).
// Verificado contra el código fuente real de ccusage (D28): burnRate.tokensPerMinute
// = totalTokens/minutos del bloque (mismo cálculo que haríamos nosotros, se
// reusa); projection.remainingMinutes es puro reloj, no depende del ritmo —
// por eso AUTO solo tiene sentido con el sensor oficial (rate_limits sí mide
// consumo real de cupo).
// ─────────────────────────────────────────────────────────────────────────────

const PACE_WINDOW_MS = 5 * 60 * 1000; // ventana reciente para PACE
const PACE_MIN_BLOCK_ELAPSED_MIN = 1; // mínimo del bloque antes de fiarse de blockAvg
const PACE_MIN_SPAN_MIN = 0.5; // mínimo span de ticks antes de fiarse de recentRate
const AUTONOMY_WINDOW_MS = 10 * 60 * 1000; // ventana reciente para AUTO
const AUTONOMY_MIN_SPAN_MIN = 2; // mínimo span real antes de fiarse del ritmo

let recentTicks = []; // { recvAt, tokens } — alimentado por onBurnTick
let recentPct = []; // { recvAt, pct } — alimentado por onSensorUpdate
let footerMetric =
  localStorage.getItem("cc-autobahn.footerMetric") === "autonomy"
    ? "autonomy"
    : "pace";

let lastGearHit = null; // último modelo activo pintado, para saber si "cambió de marcha"

/** Ilumina la marcha del selector PRND según el modelo activo (O/S/H/F).
 *  Desliza el marcador hasta la letra activa y, si cambió de marcha respecto
 *  a la anterior, dispara un pulso de glow (D-review: animar el cambio en
 *  vez de solo cambiar color en seco). */
function setGear(models) {
  if (!Array.isArray(models) || models.length === 0) return;
  const order = ["opus", "sonnet", "haiku", "fable"];
  const hit = order.find((m) =>
    models.some((id) => String(id).toLowerCase().includes(m))
  );
  if (!hit) return;

  let activeEl = null;
  document.querySelectorAll(".gear .g").forEach((el) => {
    const isActive = el.dataset.model === hit;
    el.classList.toggle("active", isActive);
    if (isActive) activeEl = el;
  });

  const gearEl = document.getElementById("gear");
  const marker = document.getElementById("gear-marker");
  if (activeEl && gearEl && marker) {
    // translateY relativo al propio .gear — robusto ante cambios de tamaño
    // de fuente/gap, no depende de asumir una altura de fila fija.
    const targetY = activeEl.offsetTop + activeEl.offsetHeight / 2;
    marker.style.transform = `translateY(${targetY}px)`;
  }

  if (hit !== lastGearHit && lastGearHit !== null && activeEl) {
    activeEl.classList.remove("pulse");
    // Forzar reflow para poder re-disparar la animación si vuelve a la misma letra.
    void activeEl.offsetWidth;
    activeEl.classList.add("pulse");
    activeEl.addEventListener(
      "animationend",
      () => activeEl.classList.remove("pulse"),
      { once: true }
    );
  }
  lastGearHit = hit;
}

/** Barra de autonomía + texto + gear desde la PROYECCIÓN de ccusage (estimado). */
function applyEstimated(block) {
  const remaining = Number(block?.projection?.remainingMinutes);
  document.getElementById("autonomie").textContent = `EST ${formatHMin(remaining)}`;
  const filled = Number.isFinite(remaining)
    ? Math.max(
        0,
        Math.min(SEGMENT_COUNT, Math.round((SEGMENT_COUNT * remaining) / WINDOW_MIN))
      )
    : 0;
  buildSegments(filled);
  setGear(block?.models);
}

/** Actualiza la cuenta atrás de autonomía hasta el reset oficial de 5h. Sigue
 *  contando aunque `sensorConnected` sea false momentáneamente — el reset
 *  conocido no deja de ser válido solo porque el sensor calle un rato. */
function refreshAutonomie() {
  if (fiveHourResetsAtMs <= 0) return;
  const remainMin = (fiveHourResetsAtMs - Date.now()) / 60000;
  document.getElementById("autonomie").textContent =
    remainMin > 0 ? formatHMin(remainMin) : "—";
}

/** Pinta los datos del bloque activo de ccusage. odo/trip/avg siempre; los
 *  derivados (segments/autonomie/gear) solo si NO hay sensor oficial conectado. */
function onBlocksUpdate(block) {
  // block = Block camelCase de engine.rs (totalTokens, costUsd, projection, models, startTime).
  // Si rota el bloque de 5h (id nuevo), el buffer PACE del bloque anterior ya
  // no es comparable — limpiarlo evita mezclar "reciente" de un bloque con la
  // media de otro (hallado en revisión: info engañosa aunque rara, D28).
  if (lastBlock && block?.id && lastBlock.id !== block.id) {
    recentTicks = [];
  }
  lastBlock = block;
  const tokens = Number(block?.totalTokens) || 0;
  document.getElementById("odo").textContent = formatTokens(tokens);

  const startedAt = block?.startTime ? Date.parse(block.startTime) : NaN;
  if (Number.isFinite(startedAt)) {
    document.getElementById("session-time").textContent = formatDurationMs(
      Date.now() - startedAt
    );
  }

  const costUsd = Number(block?.costUsd) || 0;
  const perMtok = tokens > 0 ? (costUsd / tokens) * 1e6 : 0;
  document.getElementById("avg").textContent = `$${perMtok.toFixed(2)}`;

  // Solo si NUNCA hubo sensor oficial — una vez que lo hubo, una pausa
  // momentánea no debe hacer que ccusage pise el dato oficial (D-review).
  if (!everSensorConnected) applyEstimated(block);
}

/** Dato OFICIAL del statusLine: sobreescribe segments/autonomie/gear/warn. */
function onSensorUpdate(p) {
  sensorConnected = true;
  everSensorConnected = true;
  const pct = Number.isFinite(p?.fiveHourPct) ? Math.max(0, Math.min(100, p.fiveHourPct)) : 0;
  // Segmentos = autonomía RESTANTE, no gastada (depósito que se vacía, no que
  // se llena) — coherente con applyEstimated() y con el icono de surtidor.
  buildSegments(Math.round((SEGMENT_COUNT * (100 - pct)) / 100));
  fiveHourResetsAtMs = p?.fiveHourResetsAt ? Number(p.fiveHourResetsAt) * 1000 : 0;
  refreshAutonomie();
  if (p?.modelId) setGear([p.modelId]);
  // Tinte de reserva: seven_day > 80% → borde rojo (testigo W203).
  document
    .querySelector(".screen")
    .classList.toggle("warn", (Number(p?.sevenDayPct) || 0) > 80);
  // Buffer deslizante para la métrica AUTO del footer (ver renderFooterMetric).
  if (Number.isFinite(pct)) {
    recentPct.push({ recvAt: Date.now(), pct });
  }
  renderFooterMetric();
}

/** PACE: % de diferencia entre el ritmo reciente (5 min, SOLO output tokens,
 *  de `burn-tick`) y la media de output del bloque. NO usa
 *  `burnRate.tokensPerMinute` de ccusage (D28 lo probó en vivo y falló: ese
 *  campo suma input+output+caché — `cache_read_tokens` puede ser enorme en
 *  sesiones largas por el reuso de contexto, dejando SIEMPRE el reciente
 *  (output puro) por debajo, cerca de -100% sin importar la actividad real).
 *  Se calcula la media del bloque a mano con `tokenCounts.outputTokens`, la
 *  MISMA magnitud que `burn-tick` — comparación de peras con peras. */
function computePace() {
  const now = Date.now();
  recentTicks = recentTicks.filter((t) => now - t.recvAt <= PACE_WINDOW_MS);
  const outputTokens = Number(lastBlock?.tokenCounts?.outputTokens);
  const startedAt = lastBlock?.startTime ? Date.parse(lastBlock.startTime) : NaN;
  if (
    !Number.isFinite(outputTokens) ||
    !Number.isFinite(startedAt) ||
    recentTicks.length === 0
  ) {
    return null;
  }
  // Bloque recién empezado: dividir por un elapsed casi cero infla blockAvg
  // artificialmente (mismo tipo de ruido que ya se evita en AUTO).
  const elapsedMin = (now - startedAt) / 60000;
  if (elapsedMin < PACE_MIN_BLOCK_ELAPSED_MIN) return null;
  const blockAvg = outputTokens / elapsedMin;
  if (blockAvg <= 0) return null;

  const totalTokens = recentTicks.reduce((sum, t) => sum + t.tokens, 0);
  const spanMin = (now - recentTicks[0].recvAt) / 60000;
  // Un solo tick muy reciente infla recentRate igual de artificialmente.
  if (spanMin < PACE_MIN_SPAN_MIN) return null;
  const recentRate = totalTokens / spanMin;
  return ((recentRate - blockAvg) / blockAvg) * 100;
}

/** AUTO: minutos restantes reproyectando la TENDENCIA reciente del %oficial
 *  (rate_limits.five_hour), no la proyección lineal de ccusage (que es solo
 *  reloj, D28). `null` sin sensor, sin muestras suficientes, ritmo <= 0 (no
 *  tiene sentido "tiempo hasta agotar" si no estás consumiendo), o ventana ya
 *  debería haber reseteado (fiveHourResetsAtMs stale).
 *  TECHO DURO (D-review, hallado con datos reales: 85% usado, reset en 16 min
 *  reales): la reproyección por ritmo NUNCA puede superar el tiempo real hasta
 *  `fiveHourResetsAtMs` — ese reset ocurre igual, uses o no el 100% de cupo.
 *  Sin este cap, un ritmo lento mostraría más autonomía de la que existe de
 *  verdad (info engañosa). */
function computeAdjustedAutonomy() {
  if (!sensorConnected) return null;
  const now = Date.now();
  recentPct = recentPct.filter((p) => now - p.recvAt <= AUTONOMY_WINDOW_MS);
  if (recentPct.length < 2) return null;
  const oldest = recentPct[0];
  const newest = recentPct[recentPct.length - 1];
  const spanMin = (newest.recvAt - oldest.recvAt) / 60000;
  if (spanMin < AUTONOMY_MIN_SPAN_MIN) return null;
  const ratePerMin = (newest.pct - oldest.pct) / spanMin;
  if (ratePerMin <= 0) return null;
  let minutesLeft = (100 - newest.pct) / ratePerMin;

  if (fiveHourResetsAtMs > 0) {
    const wallClockRemainingMin = (fiveHourResetsAtMs - now) / 60000;
    if (wallClockRemainingMin <= 0) return null; // reset ya debería haber pasado
    minutesLeft = Math.min(minutesLeft, wallClockRemainingMin);
  }
  return minutesLeft;
}

/** Repinta el footer según la métrica activa (PACE/AUTO, ver footerMetric). */
function renderFooterMetric() {
  const label = document.getElementById("footer-metric-label");
  const value = document.getElementById("footer-metric-value");
  if (footerMetric === "autonomy") {
    label.textContent = "AUTO";
    const minutesLeft = computeAdjustedAutonomy();
    value.textContent = minutesLeft == null ? "—" : formatHMin(minutesLeft);
  } else {
    label.textContent = "PACE";
    const deltaPct = computePace();
    if (deltaPct == null) {
      value.textContent = "—";
    } else {
      const arrow = deltaPct >= 0 ? "▲" : "▼";
      const sign = deltaPct >= 0 ? "+" : "";
      value.textContent = `${arrow} ${sign}${Math.round(deltaPct)}%`;
    }
  }
}

/** Click en el footer alterna PACE/AUTO, persistido en localStorage. */
function wireFooterToggle() {
  document.getElementById("footer-metric").onclick = () => {
    footerMetric = footerMetric === "pace" ? "autonomy" : "pace";
    localStorage.setItem("cc-autobahn.footerMetric", footerMetric);
    renderFooterMetric();
  };
}

/** Conexión del sensor. Si NUNCA hubo dato oficial, cae a la proyección
 *  "EST". Si ya lo hubo, una desconexión momentánea (idle normal, sin
 *  renderizado de Claude Code) se CONGELA tal cual — saltar a la proyección
 *  de ccusage aquí es un sistema de ventana de 5h independiente y el salto
 *  se veía como un número absurdo (ej. oficial "0h17" → "EST 4h31" de
 *  ccusage, hallado en revisión). */
function onSensorState(p) {
  sensorConnected = !!p?.connected;
  if (sensorConnected) return;
  if (everSensorConnected) return; // congelado: no tocar nada
  document.querySelector(".screen").classList.remove("warn");
  if (lastBlock) applyEstimated(lastBlock);
  else {
    document.getElementById("autonomie").textContent = "EST —";
    buildSegments(0);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// CHECK ENGINE overlay (D9, Fase 4): sin ccusage/npx/bunx en PATH no hay datos.
// Mismo patrón que el overlay del sensor: estado inicial via comando (evita la
// carrera contra el evento) + botón que dispara la instalación.
// ─────────────────────────────────────────────────────────────────────────────

let engineInvoke = null;

const ENGINE_DEFAULT_BODY =
  "No se encontró ccusage (ni global, ni npx, ni bunx) en PATH.\n" +
  "Sin motor no hay datos de consumo.";

function showEngineOverlay(show) {
  document.getElementById("engine-overlay").hidden = !show;
  if (show) setEngineBody(ENGINE_DEFAULT_BODY); // reset tras un error previo
}

function setEngineBody(text) {
  document.getElementById("engine-body").textContent = text;
}

async function onInstallEngineClick() {
  if (!engineInvoke) return;
  const btn = document.getElementById("engine-install-btn");
  if (btn.disabled) return; // doble-click: instalador ya en curso
  btn.disabled = true;
  setEngineBody(
    "Instalando Bun (curl -fsSL https://bun.sh/install | bash)…\nEsto tarda unos segundos."
  );
  try {
    const label = await engineInvoke("install_bun");
    setEngineBody(`Motor detectado (${label}). Arrancando…`);
    showEngineOverlay(false); // blocks-update/engine-detected lo confirman en breve
  } catch (e) {
    setEngineBody(String(e));
    btn.disabled = false;
  }
}

async function wireEngineOverlay() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { invoke } = await import("@tauri-apps/api/core");
  engineInvoke = invoke;
  document.getElementById("engine-install-btn").onclick = onInstallEngineClick;
  try {
    const present = await engineInvoke("engine_status");
    showEngineOverlay(!present);
  } catch (e) {
    console.error("[engine] engine_status:", e);
  }
}

/**
 * Wire the backend engine events (see src-tauri/src/engine.rs + burn.rs).
 * Guarded: under plain `vite` (no Tauri) there is no IPC, so we skip silently.
 */
async function wireEngine() {
  if (!("__TAURI_INTERNALS__" in window)) return; // running outside Tauri
  const { listen } = await import("@tauri-apps/api/event");

  listen("engine-detected", () => showEngineOverlay(false));
  listen("engine-missing", () => showEngineOverlay(true));
  listen("engine-error", (e) => console.error("[engine] error:", e.payload));
  listen("blocks-idle", () => console.info("[engine] sin bloque activo"));
  listen("blocks-update", (e) => {
    console.info("[engine] blocks-update:", e.payload);
    showEngineOverlay(false);
    onBlocksUpdate(e.payload);
  });
  listen("burn-tick", (e) => {
    console.info("[burn] tok/s por respuesta:", e.payload);
    onBurnTick(e.payload);
  });
  listen("sensor-update", (e) => {
    console.info("[sensor] oficial:", e.payload);
    onSensorUpdate(e.payload);
  });
  listen("sensor-state", (e) => {
    console.info("[sensor] state:", e.payload);
    onSensorState(e.payload);
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// UI de consentimiento del sensor (D12): conectar/desconectar el statusLine.
// Muta ~/.claude/settings.json desde el backend; el overlay pide confirmación
// con la vista previa (backup + chain) antes de escribir.
// ─────────────────────────────────────────────────────────────────────────────

let sensorInvoke = null;
let sensorInstalled = false;

async function wireSensorUi() {
  if (!("__TAURI_INTERNALS__" in window)) return; // fuera de Tauri, nada que hacer
  const { invoke } = await import("@tauri-apps/api/core");
  sensorInvoke = invoke;

  document.getElementById("sensor-connect").onclick = onConnectClick;
  document.getElementById("sensor-disconnect").onclick = onDisconnectClick;
  document.getElementById("sensor-cancel").onclick = cancelPreview;

  refreshSensorStatus();
}

async function refreshSensorStatus() {
  if (!sensorInvoke) return;
  try {
    const st = await sensorInvoke("sensor_status");
    sensorInstalled = !!st.installed;
    document.getElementById("sensor-disconnect").hidden = !sensorInstalled;
    showSensorOverlay(!sensorInstalled);
  } catch (e) {
    console.error("[sensor] status:", e);
  }
}

function showSensorOverlay(show) {
  document.getElementById("sensor-overlay").hidden = !show;
  if (!show) return;
  setSensorBody(
    "Conecta el sensor para los rate_limits oficiales (ventana 5 h / 7 d).\n" +
      "Modifica ~/.claude/settings.json con backup y rollback.\n" +
      "Tu statusLine actual se conserva (chain)."
  );
  const connect = document.getElementById("sensor-connect");
  connect.textContent = "Conectar";
  connect.onclick = onConnectClick;
  document.getElementById("sensor-cancel").hidden = true;
}

function setSensorBody(text) {
  document.getElementById("sensor-body").textContent = text;
}

async function onConnectClick() {
  if (!sensorInvoke) return;
  try {
    const p = await sensorInvoke("sensor_preview_install");
    const prev = p.prevStatusLine
      ? "Tu statusLine actual se conserva y seguirá pintándose (chain)."
      : "No tienes statusLine previo; el sensor usará una línea por defecto.";
    setSensorBody(
      `Se escribirá statusLine en settings.json.\n${prev}\nBackup: ${p.backupPath}\n\n` +
        "Si algo falla: borra statusLine o restaura el backup."
    );
    const connect = document.getElementById("sensor-connect");
    connect.textContent = "Confirmar";
    connect.onclick = doInstall;
    document.getElementById("sensor-cancel").hidden = false;
  } catch (e) {
    setSensorBody("No se pudo generar la vista previa: " + e);
  }
}

async function doInstall() {
  try {
    await sensorInvoke("install_sensor");
    refreshSensorStatus();
  } catch (e) {
    setSensorBody("Error al instalar: " + e + "\n(settings intacto — rollback automático)");
  }
}

function cancelPreview() {
  showSensorOverlay(!sensorInstalled);
}

async function onDisconnectClick() {
  try {
    await sensorInvoke("uninstall_sensor");
    refreshSensorStatus();
  } catch (e) {
    setSensorBody("Error al desconectar: " + e);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// Botón PIN (D24): fija el panel abierto pese a perder el foco. El hide-on-blur
// vive en Rust (main.rs); aquí solo se avisa del estado con `set_pinned`.
// ─────────────────────────────────────────────────────────────────────────────

async function wirePinButton() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { invoke } = await import("@tauri-apps/api/core");
  const btn = document.getElementById("pin-btn");
  let pinned = false;
  btn.onclick = () => {
    pinned = !pinned;
    btn.classList.toggle("on", pinned);
    invoke("set_pinned", { value: pinned }).catch((e) =>
      console.error("[pin] set_pinned:", e)
    );
  };
}

function init() {
  // Barra de autonomía vacía hasta el primer blocks-update (sin datos aún).
  buildSegments(0);
  tickClock();
  setInterval(tickClock, 1000);
  wireEngineOverlay();
  wireEngine();
  wireSensorUi();
  wirePinButton();
  wireFooterToggle();
  renderFooterMetric();
  setGear(["opus"]); // posiciona el marcador contra la marcha por defecto del HTML
  requestAnimationFrame(burnFrame); // arranca en ralentí (pos=0), fiel al coche
}

window.addEventListener("DOMContentLoaded", init);
