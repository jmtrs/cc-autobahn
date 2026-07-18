// Trip-computer readouts — data from ccusage's active block (blocks-update),
// overwritten with OFFICIAL data from the statusLine sensor once it connects
// (D11: official data is never presented as estimated). Wired in Phase 3
// Track A/B.

import { formatDurationMs, formatHMin, formatModelCode, formatTokens } from "./format.js";
import { renderFooterMetric } from "./footer-metric.js";
import { hintOnHover, setHeaderHint } from "./header-hint.js";
import { state } from "./telemetry-state.js";

export const SEGMENT_COUNT = 12;
const WINDOW_MIN = 300; // 5h billing window, in minutes

let lastGearHit = null; // last active model painted, to know if the "gear changed"
let lastCustomLabel = ""; // sticky: last detected non-Claude code, for the "C" slot's hover hint

// Model badge, Mercedes trim-nameplate style — one per active model (D-review naming session).
// User-editable (click the badge); custom text persists per model in localStorage.
const NAMEPLATES = {
  opus: "CC 500",
  sonnet: "CC 320",
  haiku: "CC 220 CDI",
  fable: "CC 63 AMG",
};
// Per-slot hover labels for the gear column (wireTripComputerHints) — plain
// model names, "custom" resolved dynamically at hover time instead (varies).
const GEAR_LABELS = { opus: "Opus", sonnet: "Sonnet", haiku: "Haiku", fable: "Fable" };
const NAMEPLATE_STORAGE_KEY = "cc-autobahn.nameplates";
let currentGearHit = null; // which model key the visible nameplate belongs to, for saving edits

function loadNameplateOverrides() {
  try {
    return JSON.parse(localStorage.getItem(NAMEPLATE_STORAGE_KEY)) || {};
  } catch {
    return {};
  }
}

function getNameplate(hit) {
  return loadNameplateOverrides()[hit] || NAMEPLATES[hit];
}

/** Build the autonomie segment bar (fuel-gauge style). */
export function buildSegments(filled) {
  const bar = document.getElementById("segments");
  bar.innerHTML = "";
  for (let i = 0; i < SEGMENT_COUNT; i++) {
    const seg = document.createElement("div");
    seg.className = i < filled ? "seg on" : "seg";
    bar.appendChild(seg);
  }
}

/** Lights up the PRND selector gear according to the active model (O/S/H/F,
 *  plus a 5th "C" slot for anything that isn't one of the 4 — e.g. a
 *  Claude-compatible proxy like GLM-5, D-review). Slides the marker to the
 *  active letter and, if it changed gear relative to the previous one,
 *  triggers a glow pulse (D-review: animate the change instead of just
 *  switching color abruptly). */
export function setGear(models) {
  if (!Array.isArray(models) || models.length === 0) return;
  const order = ["opus", "sonnet", "haiku", "fable"];
  const hit = order.find((m) =>
    models.some((id) => String(id).toLowerCase().includes(m))
  );
  // No known model matched: fall back to the "custom" slot, identified by
  // the first model id reported (same "first match wins" rule as `hit`,
  // not a per-session breakdown — one physical selector, one value).
  const customId = hit ? null : models.find((id) => id);
  if (!hit && !customId) return;
  const gearKey = hit || "custom";
  if (customId) lastCustomLabel = formatModelCode(customId);

  currentGearHit = gearKey;
  const nameplateEl = document.getElementById("nameplate");
  // Don't overwrite mid-edit — the user is typing (D-review: a live model
  // tick landing while editing would clobber the in-progress text).
  if (nameplateEl && nameplateEl.contentEditable !== "true") {
    // Custom slot shows the real detected model code (e.g. "GLM5") instead
    // of a decorative trim name — "custom" isn't a fixed model identity like
    // the other 4, so there's no stable NAMEPLATES entry for it.
    nameplateEl.textContent = hit ? getNameplate(hit) : formatModelCode(customId);
  }

  let activeEl = null;
  document.querySelectorAll(".gear .g").forEach((el) => {
    const isActive = el.dataset.model === gearKey;
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

  if (gearKey !== lastGearHit && lastGearHit !== null && activeEl) {
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
  lastGearHit = gearKey;
}

/** Click the nameplate badge to rewrite it for the current model. Empty
 *  input reverts to the built-in default (D-review: needs an escape hatch,
 *  otherwise a typo is permanent). Persisted per model so switching gears
 *  doesn't lose the customization. */
export function wireNameplateEdit() {
  const el = document.getElementById("nameplate");
  // No `title` (D-review): a native browser tooltip is dark-gray/sans-serif
  // OS chrome, breaks the amber VFD look with no CSS-reachable fix — same
  // reason the MFD/PIN buttons and PACE/AUTO toggle don't have one either.
  // header-hint.js replaces it.
  hintOnHover(el, "Click to rename this model's badge");
  el.onclick = () => {
    // The "custom" slot isn't one fixed model identity (D-review) — it can
    // be a different proxy from one block to the next, so a saved rename
    // would go stale exactly where accuracy matters most. Not editable.
    if (currentGearHit === "custom") return;
    el.contentEditable = "true";
    el.focus();
    const range = document.createRange();
    range.selectNodeContents(el);
    const sel = window.getSelection();
    sel.removeAllRanges();
    sel.addRange(range);
  };
  const commit = () => {
    if (el.contentEditable !== "true") return;
    el.contentEditable = "false";
    if (!currentGearHit) return;
    const overrides = loadNameplateOverrides();
    const value = el.textContent.trim().toUpperCase();
    if (!value || value === NAMEPLATES[currentGearHit]) {
      delete overrides[currentGearHit];
    } else {
      overrides[currentGearHit] = value;
    }
    localStorage.setItem(NAMEPLATE_STORAGE_KEY, JSON.stringify(overrides));
    el.textContent = getNameplate(currentGearHit);
  };
  el.onblur = commit;
  el.onkeydown = (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      el.blur();
    } else if (e.key === "Escape") {
      el.textContent = getNameplate(currentGearHit);
      el.blur();
    }
  };
}

/** Segment count for `min` minutes remaining out of the 5h window — used only
 *  by the estimated fallback (D40: official mode reads quota %, not time; see
 *  onSensorUpdate()). Segments and the "autonomie" text must still read the
 *  same unit within a source (D23). */
function segmentsForMinutes(min) {
  return Math.max(0, Math.min(SEGMENT_COUNT, Math.round((SEGMENT_COUNT * min) / WINDOW_MIN)));
}

/** Autonomy bar + text + gear from ccusage's PROJECTION (estimated). */
export function applyEstimated(block) {
  const remaining = Number(block?.projection?.remainingMinutes);
  document.getElementById("autonomie").textContent = `EST ${formatHMin(remaining)}`;
  buildSegments(Number.isFinite(remaining) ? segmentsForMinutes(remaining) : 0);
  setGear(block?.models);
}

/** Paints the bar + text for quota mode, from the last known official data
 *  (no fresh payload needed — click-toggle and the clock tick both just
 *  repaint what's already in `state`). Default view is quota remaining
 *  (D40); `state.autonomieShowTime` (click-toggle, D40-toggle) swaps both to
 *  the reset time instead, since D40 demoted it to "redundant with the
 *  clock" but users still want an on-demand way to see it without losing
 *  the quota view permanently. Falls back to quota if the toggle is on but
 *  `resetsAt` never arrived (partial payload) — same "don't force a value
 *  that isn't known" rule as everywhere else in this file. */
function paintQuotaGauge() {
  if (state.autonomieShowTime && state.fiveHourResetsAtMs > 0) {
    const remainMin = (state.fiveHourResetsAtMs - Date.now()) / 60000;
    document.getElementById("autonomie").textContent =
      remainMin > 0 ? formatHMin(remainMin) : "—";
    buildSegments(segmentsForMinutes(Math.max(0, remainMin)));
    return;
  }
  const pct = state.fiveHourPct;
  buildSegments(Math.round((SEGMENT_COUNT * (100 - pct)) / 100));
  document.getElementById("autonomie").textContent = `${Math.round(100 - pct)}%`;
}

/** Re-paints the "autonomie" row on each clock tick (D40). In official mode
 *  neither view changes on its own between pushes — this just re-asserts
 *  whichever one is toggled on so the tick never clobbers it. In estimated
 *  mode (no sensor ever connected) falls back to the time-until-reset
 *  countdown, unchanged from before D40: keeps counting even while
 *  `sensorConnected` is momentarily false — the known reset doesn't stop
 *  being valid just because the sensor is quiet for a while. */
export function refreshAutonomie() {
  if (state.everQuotaConnected) {
    paintQuotaGauge();
    return;
  }
  if (state.fiveHourResetsAtMs <= 0) return;
  const remainMin = (state.fiveHourResetsAtMs - Date.now()) / 60000;
  document.getElementById("autonomie").textContent =
    remainMin > 0 ? formatHMin(remainMin) : "—";
}

/** Click-toggle for the quota gauge (D40-toggle): flips between quota
 *  remaining and time-until-reset, both bar and text together (never a mix,
 *  D40). No-op in estimated mode — there's no quota to toggle to, so the
 *  row is left showing ccusage's time projection either way. */
export function toggleAutonomieView() {
  if (!state.everQuotaConnected) return;
  state.autonomieShowTime = !state.autonomieShowTime;
  paintQuotaGauge();
}

/** Paints the ccusage active block's data. odo/trip/avg always; the
 *  derived ones (segments/autonomie/gear) only if there is NO official sensor connected. */
export function onBlocksUpdate(block) {
  // block = camelCase Block from engine.rs (totalTokens, costUsd, projection, models, startTime).
  // If the 5h block rotates (new id), the previous block's PACE buffer is no
  // longer comparable — clearing it avoids mixing "recent" data from one
  // block with another's average (found in review: misleading info even if
  // rare, D28).
  if (state.lastBlock && block?.id && state.lastBlock.id !== block.id) {
    state.recentTicks = [];
  }
  state.lastBlock = block;
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

  // Only if quota data NEVER arrived — once it did, a momentary pause
  // shouldn't let ccusage override the official data (D-review). Gated on
  // `everQuotaConnected`, not `everSensorConnected` (D40-fix): a non-Pro/Max
  // user's sensor connects but never carries quota, so ccusage's time
  // estimate must keep serving as the permanent fallback for them, not just
  // until the sensor's first (quota-less) payload.
  if (!state.everQuotaConnected) applyEstimated(block);
  // Lets Page 2 (limits-page.js) keep its instant/avg burn rate live while visible.
  document.dispatchEvent(new Event("telemetry-tick"));
}

/** OFFICIAL data from the statusLine: overwrites segments/autonomie/gear/warn. */
export function onSensorUpdate(p) {
  state.sensorConnected = true;
  state.everSensorConnected = true;
  const pctFinite = Number.isFinite(p?.fiveHourPct);
  const pct = pctFinite ? Math.max(0, Math.min(100, p.fiveHourPct)) : 0;
  state.fiveHourResetsAtMs = p?.fiveHourResetsAt ? Number(p.fiveHourResetsAt) * 1000 : 0;
  // Segments + "autonomie" text now read QUOTA remaining (100 - pct), not
  // time-until-reset (D40, supersedes D39): time is redundant with the clock
  // already on screen, so it's demoted to the estimated-only fallback (see
  // applyEstimated()). If `used_percentage` didn't arrive with this payload
  // (a real, tolerated partial shape — see sensor::mod.rs's
  // `tolerates_partial_rate_limits`), leave segments/text as they were rather
  // than forcing a value on incomplete data.
  if (pctFinite) {
    state.everQuotaConnected = true;
    state.fiveHourPct = pct;
    paintQuotaGauge();
  }
  if (p?.modelId) setGear([p.modelId]);
  // Official 7d rate-limit window — full numbers live on Page 2 (limits-page.js);
  // the border tint here stays as the always-visible "check engine"-style warning.
  const sevenDayPct = Number(p?.sevenDayPct) || 0;
  state.sevenDayPct = sevenDayPct;
  state.sevenDayResetsAtMs = p?.sevenDayResetsAt ? Number(p.sevenDayResetsAt) * 1000 : 0;
  document.querySelector(".screen").classList.toggle("warn", sevenDayPct > 80);
  // Sliding buffer for the footer's AUTO metric (see footer-metric.js).
  if (Number.isFinite(pct)) {
    state.recentPct.push({ recvAt: Date.now(), pct });
  }
  renderFooterMetric();
  document.dispatchEvent(new Event("telemetry-tick"));
}

/** Sensor connection. If official data NEVER arrived, falls back to the
 *  "EST" projection. If it already did, a momentary disconnection (normal
 *  idle, no Claude Code rendering) FREEZES as-is — jumping to ccusage's
 *  projection here is an independent 5h window system and the jump
 *  looked like an absurd number (e.g. official "0h17" → ccusage's
 *  "EST 4h31", found in review). */
export function onSensorState(p) {
  state.sensorConnected = !!p?.connected;
  if (state.sensorConnected) return;
  if (state.everQuotaConnected) return; // frozen: don't touch anything
  document.querySelector(".screen").classList.remove("warn");
  if (state.lastBlock) applyEstimated(state.lastBlock);
  else {
    document.getElementById("autonomie").textContent = "EST —";
    buildSegments(0);
  }
}

/** Header-hint wiring for Page 0's static glyphs and numbers whose meaning
 *  isn't fully covered by their `.unit` label (D-review). The `.row.gauge`
 *  hint covers the fuel icon + segment bar + "3h12" text as one zone —
 *  they're one gauge, not three separate things to explain. `#burn`/`#avg`
 *  get one despite already having a unit label: D8/D11 are this project's
 *  most-documented points of confusion (tok/s isn't live, cost is estimated). */
export function wireTripComputerHints() {
  // Per-letter hints (not one static hintOnHover on the whole .gear): each
  // slot names its own model even when it isn't the active one, and the
  // active slot is called out explicitly. "custom" has no fixed identity
  // (D-review) so its label is resolved live from the last detected id.
  document.querySelectorAll(".gear .g").forEach((el) => {
    const model = el.dataset.model;
    el.addEventListener("mouseenter", () => {
      const label =
        model === "custom" ? lastCustomLabel || "Custom (non-Claude model)" : GEAR_LABELS[model];
      setHeaderHint(currentGearHit === model ? `Active model: ${label}` : label);
    });
    el.addEventListener("mouseleave", () => setHeaderHint(""));
  });
  const gaugeRow = document.querySelector(".row.gauge");
  hintOnHover(gaugeRow, "5h quota remaining — click for reset time");
  gaugeRow.onclick = toggleAutonomieView;
  hintOnHover(
    document.getElementById("burn"),
    "Per-response rate"
  );
  hintOnHover(document.getElementById("avg"), "Estimated cost, not official");
  hintOnHover(document.getElementById("odo"), "Total tokens since this block started");
  hintOnHover(document.getElementById("session-time"), "Elapsed time in this 5h block");
}
