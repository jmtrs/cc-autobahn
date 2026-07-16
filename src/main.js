// cc-autobahn — frontend shell.
// Paints the W203 cluster and listens to the backend sensors (engine.rs + burn.rs).
// Clock + segment bar are static; speedometer = tok/s per response with a
// physical spring (D8): jumps when a turn completes and decays to idle. It is
// NOT instantaneous — the JSONL only reports usage when the turn closes (see D8/D11).

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
  // Keeps counting even while the sensor is momentarily silent — the known
  // reset (fiveHourResetsAtMs) is still valid regardless (see D-review).
  refreshAutonomie();
  renderFooterMetric(); // trims the PACE/AUTO buffers even if no new event arrives
}

// ─────────────────────────────────────────────────────────────────────────────
// Speedometer — physical spring (D8).
// The needle jumps to the tok/s of the completed turn and decays with a spring to idle.
// This is the HONEST reading: "tok/s per response", never "instantaneous" (D11).
// ─────────────────────────────────────────────────────────────────────────────

const burn = {
  target: 0, // target tok/s (last tick, or decaying while idle)
  pos: 0, // displayed value (animated by the spring)
  vel: 0, // spring velocity → mechanical overshoot
  lastTickAt: 0, // performance.now() of the last burn-tick
  SPRING_K: 0.2, // spring stiffness
  SPRING_DAMP: 0.75, // damping (>0 = underdamped, overshoot)
  IDLE_AFTER_MS: 2000, // no fresh tick → idle
  IDLE_DECAY: 0.95, // per frame, the target decays toward 0
};

/** Formats tok/s VFD-style: "7.2", "55", "1.5k". */
function formatTps(tps) {
  if (tps < 0.5) return "0";
  if (tps < 10) return tps.toFixed(1);
  if (tps < 1000) return Math.round(tps).toString();
  return (tps / 1000).toFixed(1) + "k";
}

/** Spring animation loop. Always runs (also to decay). */
function burnFrame(now) {
  // Idle: no fresh tick, the target decays toward 0.
  if (burn.lastTickAt && now - burn.lastTickAt > burn.IDLE_AFTER_MS) {
    burn.target *= burn.IDLE_DECAY;
    if (burn.target < 0.5) burn.target = 0;
  }
  // Spring integration: force → velocity (damped) → position.
  const force = (burn.target - burn.pos) * burn.SPRING_K;
  burn.vel = (burn.vel + force) * burn.SPRING_DAMP;
  burn.pos += burn.vel;
  if (burn.pos < 0) burn.pos = 0;

  document.getElementById("burn").textContent = formatTps(burn.pos);
  requestAnimationFrame(burnFrame);
}

/** Handles a burn-tick from the backend (closed turn or intermediate message, D27). */
function onBurnTick(payload) {
  // payload = { tokPerS, turnOutputTokens, turnDurationMs, messageId, timestamp }
  const tps = Number(payload?.tokPerS) || 0;
  burn.target = tps;
  burn.lastTickAt = performance.now();
  // Sliding buffer for the footer's PACE metric (see renderFooterMetric).
  const tokens = Number(payload?.turnOutputTokens) || 0;
  if (tokens > 0) {
    recentTicks.push({ recvAt: Date.now(), tokens });
  }
  renderFooterMetric();
}

// ─────────────────────────────────────────────────────────────────────────────
// Trip-computer readouts — data from ccusage's active block (blocks-update).
// Wired in Phase 3 Track A. The autonomy here is ESTIMATED (ccusage's token
// projection): the statusline sensor (Track B) overwrites it with the official
// rate_limits.five_hour once it arrives. The "EST" mark is mandatory until then.
// ─────────────────────────────────────────────────────────────────────────────

const WINDOW_MIN = 300; // 5h billing window, in minutes

/** Formats tokens VFD-style: "999", "1.5k", "850k", "1.24M", "2.1G". */
function formatTokens(n) {
  if (!(n >= 1)) return "0";
  if (n < 1e3) return String(Math.round(n));
  if (n < 1e6) return (n / 1e3).toFixed(n < 1e5 ? 1 : 0).replace(/\.0$/, "") + "k";
  if (n < 1e9) return (n / 1e6).toFixed(2) + "M";
  return (n / 1e9).toFixed(2) + "G";
}

/** Remaining minutes → "3h12" (window autonomy). */
function formatHMin(minutes) {
  if (!(minutes >= 0)) return "—";
  // Round ONCE to a whole minute before splitting into h/m: rounding each
  // part separately can produce m=60 (e.g. 119.5 → h=1, round(59.5)=60 →
  // "1h60" instead of "2h00"; bug found while reviewing the formulas).
  const total = Math.round(minutes);
  const h = Math.floor(total / 60);
  const m = total % 60;
  return `${h}h${String(m).padStart(2, "0")}`;
}

/** duration in ms → "H:MM" (time elapsed since the block started). */
function formatDurationMs(ms) {
  if (!(ms > 0)) return "0:00";
  const totalMin = Math.floor(ms / 60000);
  return `${Math.floor(totalMin / 60)}:${String(totalMin % 60).padStart(2, "0")}`;
}

// ─────────────────────────────────────────────────────────────────────────────
// Official sensor state. When the statusLine is connected, its data
// (rate_limits, model.id, effort) takes PRIORITY over the block's projection
// (D11: official data is never estimated). On disconnect, it falls back to
// the projection with the "EST" mark.
// ─────────────────────────────────────────────────────────────────────────────

let lastBlock = null; // last blocks-update, to re-apply the estimate on disconnect
// `sensorConnected` = is the sensor pushing FRESH data right now? (D-review:
// a normal pause with no Claude Code rendering sets it to false for a few
// seconds, without being a real "never connected"). `everSensorConnected` is
// sticky — once true, it never falls back to ccusage's projection again
// (independent 5h window system; the jump between the two looked
// absurd: official "0h17" → ccusage's "EST 4h31").
let sensorConnected = false; // is official data arriving from the statusLine?
let everSensorConnected = false; // did it ever connect? (sticky, see above)
let fiveHourResetsAtMs = 0; // epoch-ms of the 5h reset (countdown, refreshed by the clock)

// ─────────────────────────────────────────────────────────────────────────────
// Footer: PACE (recent pace vs. block average) / AUTO (autonomy adjusted to
// the recent pace, official sensor only). Replaces the old "LAST tok/s"
// (D27 made it ambiguous: full turn vs. intermediate message).
// Verified against ccusage's actual source code (D28): burnRate.tokensPerMinute
// = block's totalTokens/minutes (the same calculation we would do, so it's
// reused); projection.remainingMinutes is pure clock, it doesn't depend on
// pace — that's why AUTO only makes sense with the official sensor
// (rate_limits does measure real quota consumption).
// ─────────────────────────────────────────────────────────────────────────────

const PACE_WINDOW_MS = 5 * 60 * 1000; // recent window for PACE
const PACE_MIN_BLOCK_ELAPSED_MIN = 1; // minimum block elapsed before trusting blockAvg
const PACE_MIN_SPAN_MIN = 0.5; // minimum tick span before trusting recentRate
const AUTONOMY_WINDOW_MS = 10 * 60 * 1000; // recent window for AUTO
const AUTONOMY_MIN_SPAN_MIN = 2; // minimum real span before trusting the pace

let recentTicks = []; // { recvAt, tokens } — fed by onBurnTick
let recentPct = []; // { recvAt, pct } — fed by onSensorUpdate
let footerMetric =
  localStorage.getItem("cc-autobahn.footerMetric") === "autonomy"
    ? "autonomy"
    : "pace";

let lastGearHit = null; // last active model painted, to know if the "gear changed"

/** Lights up the PRND selector gear according to the active model (O/S/H/F).
 *  Slides the marker to the active letter and, if it changed gear relative
 *  to the previous one, triggers a glow pulse (D-review: animate the change
 *  instead of just switching color abruptly). */
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
    // translateY relative to .gear itself — robust against font-size/gap
    // changes, doesn't depend on assuming a fixed row height.
    const targetY = activeEl.offsetTop + activeEl.offsetHeight / 2;
    marker.style.transform = `translateY(${targetY}px)`;
  }

  if (hit !== lastGearHit && lastGearHit !== null && activeEl) {
    activeEl.classList.remove("pulse");
    // Force reflow so the animation can re-trigger if it returns to the same letter.
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

/** Autonomy bar + text + gear from ccusage's PROJECTION (estimated). */
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

/** Updates the autonomy countdown to the official 5h reset. Keeps counting
 *  even while `sensorConnected` is momentarily false — the known reset
 *  doesn't stop being valid just because the sensor is quiet for a while. */
function refreshAutonomie() {
  if (fiveHourResetsAtMs <= 0) return;
  const remainMin = (fiveHourResetsAtMs - Date.now()) / 60000;
  document.getElementById("autonomie").textContent =
    remainMin > 0 ? formatHMin(remainMin) : "—";
}

/** Paints the ccusage active block's data. odo/trip/avg always; the
 *  derived ones (segments/autonomie/gear) only if there is NO official sensor connected. */
function onBlocksUpdate(block) {
  // block = camelCase Block from engine.rs (totalTokens, costUsd, projection, models, startTime).
  // If the 5h block rotates (new id), the previous block's PACE buffer is no
  // longer comparable — clearing it avoids mixing "recent" data from one
  // block with another's average (found in review: misleading info even if
  // rare, D28).
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

  // Only if there was NEVER an official sensor — once there was one, a
  // momentary pause shouldn't let ccusage override the official data (D-review).
  if (!everSensorConnected) applyEstimated(block);
}

/** OFFICIAL data from the statusLine: overwrites segments/autonomie/gear/warn. */
function onSensorUpdate(p) {
  sensorConnected = true;
  everSensorConnected = true;
  const pct = Number.isFinite(p?.fiveHourPct) ? Math.max(0, Math.min(100, p.fiveHourPct)) : 0;
  // Segments = REMAINING autonomy, not spent (a tank that empties, not one
  // that fills) — consistent with applyEstimated() and the fuel-pump icon.
  buildSegments(Math.round((SEGMENT_COUNT * (100 - pct)) / 100));
  fiveHourResetsAtMs = p?.fiveHourResetsAt ? Number(p.fiveHourResetsAt) * 1000 : 0;
  refreshAutonomie();
  if (p?.modelId) setGear([p.modelId]);
  // Reserve tint: seven_day > 80% → red border (W203 warning light).
  document
    .querySelector(".screen")
    .classList.toggle("warn", (Number(p?.sevenDayPct) || 0) > 80);
  // Sliding buffer for the footer's AUTO metric (see renderFooterMetric).
  if (Number.isFinite(pct)) {
    recentPct.push({ recvAt: Date.now(), pct });
  }
  renderFooterMetric();
}

/** PACE: % difference between the recent pace (5 min, output tokens ONLY,
 *  from `burn-tick`) and the block's output average. Does NOT use ccusage's
 *  `burnRate.tokensPerMinute` (D28 tested it live and it failed: that
 *  field sums input+output+cache — `cache_read_tokens` can be huge in
 *  long sessions due to context reuse, ALWAYS leaving the recent value
 *  (output only) below it, close to -100% regardless of actual activity).
 *  The block average is computed by hand from `tokenCounts.outputTokens`, the
 *  SAME magnitude as `burn-tick` — comparing apples to apples. */
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
  // Block just started: dividing by an elapsed time close to zero inflates
  // blockAvg artificially (same kind of noise already avoided in AUTO).
  const elapsedMin = (now - startedAt) / 60000;
  if (elapsedMin < PACE_MIN_BLOCK_ELAPSED_MIN) return null;
  const blockAvg = outputTokens / elapsedMin;
  if (blockAvg <= 0) return null;

  const totalTokens = recentTicks.reduce((sum, t) => sum + t.tokens, 0);
  const spanMin = (now - recentTicks[0].recvAt) / 60000;
  // A single very recent tick inflates recentRate just as artificially.
  if (spanMin < PACE_MIN_SPAN_MIN) return null;
  const recentRate = totalTokens / spanMin;
  return ((recentRate - blockAvg) / blockAvg) * 100;
}

/** AUTO: minutes remaining by reprojecting the recent TREND of the official
 *  % (rate_limits.five_hour), not ccusage's linear projection (which is just
 *  clock-based, D28). `null` with no sensor, not enough samples, pace <= 0
 *  (no point in "time until exhausted" if you're not consuming), or the
 *  window should already have reset (fiveHourResetsAtMs stale).
 *  HARD CAP (D-review, found with real data: 85% used, reset in 16 real
 *  minutes): the pace-based reprojection can NEVER exceed the actual time
 *  remaining until `fiveHourResetsAtMs` — that reset happens regardless of
 *  whether you use 100% of the quota or not. Without this cap, a slow pace
 *  would show more autonomy than actually exists (misleading info). */
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
    if (wallClockRemainingMin <= 0) return null; // reset should already have happened
    minutesLeft = Math.min(minutesLeft, wallClockRemainingMin);
  }
  return minutesLeft;
}

/** Repaints the footer based on the active metric (PACE/AUTO, see footerMetric). */
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

/** Clicking the footer toggles PACE/AUTO, persisted to localStorage. */
function wireFooterToggle() {
  document.getElementById("footer-metric").onclick = () => {
    footerMetric = footerMetric === "pace" ? "autonomy" : "pace";
    localStorage.setItem("cc-autobahn.footerMetric", footerMetric);
    renderFooterMetric();
  };
}

/** Sensor connection. If official data NEVER arrived, falls back to the
 *  "EST" projection. If it already did, a momentary disconnection (normal
 *  idle, no Claude Code rendering) FREEZES as-is — jumping to ccusage's
 *  projection here is an independent 5h window system and the jump
 *  looked like an absurd number (e.g. official "0h17" → ccusage's
 *  "EST 4h31", found in review). */
function onSensorState(p) {
  sensorConnected = !!p?.connected;
  if (sensorConnected) return;
  if (everSensorConnected) return; // frozen: don't touch anything
  document.querySelector(".screen").classList.remove("warn");
  if (lastBlock) applyEstimated(lastBlock);
  else {
    document.getElementById("autonomie").textContent = "EST —";
    buildSegments(0);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// CHECK ENGINE overlay (D9, Phase 4): without ccusage/npx/bunx in PATH there's
// no data. Same pattern as the sensor overlay: initial state via command
// (avoids racing against the event) + a button that triggers the install.
// ─────────────────────────────────────────────────────────────────────────────

let engineInvoke = null;

const ENGINE_DEFAULT_BODY =
  "ccusage was not found (neither global, npx, nor bunx) in PATH.\n" +
  "Without an engine there is no usage data.";

function showEngineOverlay(show) {
  document.getElementById("engine-overlay").hidden = !show;
  if (show) setEngineBody(ENGINE_DEFAULT_BODY); // reset after a previous error
}

function setEngineBody(text) {
  document.getElementById("engine-body").textContent = text;
}

async function onInstallEngineClick() {
  if (!engineInvoke) return;
  const btn = document.getElementById("engine-install-btn");
  if (btn.disabled) return; // double-click: installer already in progress
  btn.disabled = true;
  setEngineBody(
    "Installing Bun (curl -fsSL https://bun.sh/install | bash)…\nThis takes a few seconds."
  );
  try {
    const label = await engineInvoke("install_bun");
    setEngineBody(`Engine detected (${label}). Starting…`);
    showEngineOverlay(false); // blocks-update/engine-detected will confirm it shortly
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
  listen("blocks-idle", () => console.info("[engine] no active block"));
  listen("blocks-update", (e) => {
    console.info("[engine] blocks-update:", e.payload);
    showEngineOverlay(false);
    onBlocksUpdate(e.payload);
  });
  listen("burn-tick", (e) => {
    console.info("[burn] tok/s per response:", e.payload);
    onBurnTick(e.payload);
  });
  listen("sensor-update", (e) => {
    console.info("[sensor] official:", e.payload);
    onSensorUpdate(e.payload);
  });
  listen("sensor-state", (e) => {
    console.info("[sensor] state:", e.payload);
    onSensorState(e.payload);
  });
}

// ─────────────────────────────────────────────────────────────────────────────
// Sensor consent UI (D12): connect/disconnect the statusLine. Mutates
// ~/.claude/settings.json from the backend; the overlay asks for confirmation
// with a preview (backup + chain) before writing.
// ─────────────────────────────────────────────────────────────────────────────

let sensorInvoke = null;
let sensorInstalled = false;

async function wireSensorUi() {
  if (!("__TAURI_INTERNALS__" in window)) return; // outside Tauri, nothing to do
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
    "Connect the sensor for the official rate_limits (5h / 7d window).\n" +
      "Modifies ~/.claude/settings.json with backup and rollback.\n" +
      "Your current statusLine is preserved (chain)."
  );
  const connect = document.getElementById("sensor-connect");
  connect.textContent = "Connect";
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
      ? "Your current statusLine is preserved and will keep rendering (chain)."
      : "You have no previous statusLine; the sensor will use a default line.";
    setSensorBody(
      `statusLine will be written to settings.json.\n${prev}\nBackup: ${p.backupPath}\n\n` +
        "If something goes wrong: delete statusLine or restore the backup."
    );
    const connect = document.getElementById("sensor-connect");
    connect.textContent = "Confirm";
    connect.onclick = doInstall;
    document.getElementById("sensor-cancel").hidden = false;
  } catch (e) {
    setSensorBody("Could not generate the preview: " + e);
  }
}

async function doInstall() {
  try {
    await sensorInvoke("install_sensor");
    refreshSensorStatus();
  } catch (e) {
    setSensorBody("Install error: " + e + "\n(settings untouched — automatic rollback)");
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
    setSensorBody("Disconnect error: " + e);
  }
}

// ─────────────────────────────────────────────────────────────────────────────
// PIN button (D24): pins the panel open despite losing focus. The hide-on-blur
// logic lives in Rust (main.rs); here we just report the state via `set_pinned`.
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
  // Autonomy bar empty until the first blocks-update (no data yet).
  buildSegments(0);
  tickClock();
  setInterval(tickClock, 1000);
  wireEngineOverlay();
  wireEngine();
  wireSensorUi();
  wirePinButton();
  wireFooterToggle();
  renderFooterMetric();
  setGear(["opus"]); // positions the marker against the HTML's default gear
  requestAnimationFrame(burnFrame); // starts idle (pos=0), true to the car
}

window.addEventListener("DOMContentLoaded", init);
