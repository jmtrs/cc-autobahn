// Trip-computer readouts — data from ccusage's active block (blocks-update),
// overwritten with OFFICIAL data from the statusLine sensor once it connects
// (D11: official data is never presented as estimated). Wired in Phase 3
// Track A/B.

import { formatDurationMs, formatHMin, formatResetAt, formatTokens } from "./format.js";
import { renderFooterMetric } from "./footer-metric.js";
import { hintOnHover, setHeaderHint } from "./header-hint.js";
import { loadGlobalSetting, saveGlobalSetting } from "./app-settings.js";
import { claudeView } from "./provider-view.js";
import { defaultNameplate, modelSlots, resolveModelPresentation } from "./model-presentation.js";
import {
  reconcileNameplateEdit,
  recordModelActivity,
  state as appState,
} from "./telemetry-state.js";

export const SEGMENT_COUNT = 12;
const WINDOW_MIN = 300; // 5h billing window, in minutes
const WEEKLY_WINDOW_MIN = 7 * 24 * 60;

const runtimeByProvider = new Map();

function runtime(view) {
  if (!runtimeByProvider.has(view.provider)) {
    runtimeByProvider.set(view.provider, {
      lastGearHit: null,
      lastCustomLabel: "",
      currentGearHit: null,
      currentModelKey: null,
      currentModelEditable: false,
    });
  }
  return runtimeByProvider.get(view.provider);
}

function loadNameplateOverrides() {
  return loadGlobalSetting("nameplates");
}

function getNameplate(modelKey, view, fallback = null) {
  return (
    loadNameplateOverrides()[`${view.provider}:${modelKey}`] ||
    defaultNameplate(view.provider, modelKey) ||
    fallback
  );
}

/** Build the autonomie segment bar (fuel-gauge style). */
export function buildSegments(filled, view = claudeView) {
  const bar = view.element("segments");
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
export function setGear(models, view = claudeView, activity = {}) {
  if (!Array.isArray(models) || models.length === 0) return;
  const local = runtime(view);
  const presentation = models
    .map((modelId) => resolveModelPresentation(view.provider, modelId))
    .find(Boolean);
  if (!presentation) return;
  const { modelKey, slotKey: gearKey, editable } = presentation;
  if (gearKey === "custom") local.lastCustomLabel = presentation.nameplate;

  const label = getNameplate(modelKey, view, presentation.nameplate);
  const accepted = recordModelActivity({
    provider: view.provider,
    modelKey,
    label,
    sessionOrThreadId: activity.sessionOrThreadId,
    observedAtMs: activity.observedAtMs ?? Date.now(),
    sequence: activity.sequence ?? 0,
  });
  if (!accepted.providerAccepted) return;
  local.currentGearHit = gearKey;
  local.currentModelKey = modelKey;
  local.currentModelEditable = editable;
  const nameplateEl = view.chassisElement("nameplate");
  // Don't overwrite mid-edit — the user is typing (D-review: a live model
  // tick landing while editing would clobber the in-progress text).
  if (accepted.globalAccepted && nameplateEl && nameplateEl.contentEditable !== "true") {
    nameplateEl.textContent = label;
  }

  let activeEl = null;
  view.queryAll(".gear .g").forEach((el) => {
    const isActive = el.dataset.model === gearKey;
    el.classList.toggle("active", isActive);
    if (isActive) activeEl = el;
  });

  const gearEl = view.element("gear");
  const marker = view.element("gear-marker");
  if (activeEl && gearEl && marker) {
    marker.hidden = false;
    // translateY relative to .gear itself — robust against font-size/gap
    // changes, doesn't depend on assuming a fixed row height.
    const targetY = activeEl.offsetTop + activeEl.offsetHeight / 2;
    marker.style.transform = `translateY(${targetY}px)`;
  }

  if (gearKey !== local.lastGearHit && local.lastGearHit !== null && activeEl) {
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
  local.lastGearHit = gearKey;
}

/** Click the nameplate badge to rewrite it for the current model. Empty
 *  input reverts to the built-in default (D-review: needs an escape hatch,
 *  otherwise a typo is permanent). Persisted per model so switching gears
 *  doesn't lose the customization. */
export function wireNameplateEdit(views = [claudeView]) {
  const candidates = Array.isArray(views) ? views : [views];
  const el = candidates[0].chassisElement("nameplate");
  let editContext = null;
  const activeContext = () => {
    const provider = appState.global.lastActiveModel?.provider;
    const view = candidates.find((candidate) => candidate.provider === provider);
    if (!view) return null;
    const local = runtime(view);
    return local.currentModelKey
      ? { view, modelKey: local.currentModelKey, editable: local.currentModelEditable }
      : null;
  };
  // No `title` (D-review): a native browser tooltip is dark-gray/sans-serif
  // OS chrome, breaks the amber VFD look with no CSS-reachable fix — same
  // reason the MFD/PIN buttons and PACE/AUTO toggle don't have one either.
  // header-hint.js replaces it.
  hintOnHover(el, "Click to rename this model's badge");
  el.onclick = () => {
    editContext = activeContext();
    if (!editContext) return;
    // The "custom" slot isn't one fixed model identity (D-review) — it can
    // be a different proxy from one block to the next, so a saved rename
    // would go stale exactly where accuracy matters most. Not editable.
    if (!editContext.editable) {
      editContext = null;
      return;
    }
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
    if (!editContext) return;
    const { view, modelKey } = editContext;
    const overrides = loadNameplateOverrides();
    const value = el.textContent.trim().toUpperCase();
    if (!value || value === defaultNameplate(view.provider, modelKey)) {
      delete overrides[`${view.provider}:${modelKey}`];
    } else {
      overrides[`${view.provider}:${modelKey}`] = value;
    }
    saveGlobalSetting("nameplates", overrides);
    const label = getNameplate(modelKey, view);
    // A newer provider/model may have arrived while contentEditable prevented
    // its normal repaint. Persist the old override, but never roll the shared
    // header back over that newer activity.
    el.textContent = reconcileNameplateEdit(view.provider, modelKey, label);
    editContext = null;
  };
  el.onblur = commit;
  el.onkeydown = (e) => {
    if (e.key === "Enter") {
      e.preventDefault();
      el.blur();
    } else if (e.key === "Escape") {
      el.textContent = appState.global.lastActiveModel?.label ?? el.textContent;
      el.blur();
    }
  };
}

/** Segment count for `min` minutes remaining out of the 5h window — used only
 *  by the estimated fallback (D40: official mode reads quota %, not time; see
 *  onSensorUpdate()). Segments and the "autonomie" text must still read the
 *  same unit within a source (D23). */
function segmentsForMinutes(min, windowMinutes = WINDOW_MIN) {
  const duration = Number(windowMinutes) > 0 ? Number(windowMinutes) : WINDOW_MIN;
  return Math.max(0, Math.min(SEGMENT_COUNT, Math.round((SEGMENT_COUNT * min) / duration)));
}

/** Autonomy bar + text + gear from ccusage's PROJECTION (estimated). */
export function applyEstimated(block, view = claudeView) {
  const remaining = Number(block?.projection?.remainingMinutes);
  view.element("autonomie").textContent = `EST ${formatHMin(remaining)}`;
  buildSegments(Number.isFinite(remaining) ? segmentsForMinutes(remaining) : 0, view);
  setGear(block?.models, view, {
    observedAtMs: block?.observedAtMs,
    sequence: block?.sequence ?? 0,
  });
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
function paintQuotaGauge(view) {
  const state = view.state;
  if (state.rateLimitSourceQuality === "unavailable") {
    view.element("autonomie").textContent = "UNAVAILABLE";
    buildSegments(0, view);
    view.element("gauge-reset-label").textContent = "";
    return;
  }
  // Small date/time above the bar (D-review): "154h23" alone doesn't say
  // *when* the window resets, only how far away. Same reset timestamp the
  // countdown/quota text below is already derived from.
  view.element("gauge-reset-label").textContent =
    state.fiveHourResetsAtMs > 0 ? formatResetAt(state.fiveHourResetsAtMs) : "";
  const qualityPrefix = state.rateLimitSourceQuality === "stale" ? "STALE " : "";
  if (state.autonomieShowTime && state.fiveHourResetsAtMs > 0) {
    const remainMin = (state.fiveHourResetsAtMs - Date.now()) / 60000;
    view.element("autonomie").textContent =
      remainMin > 0 ? `${qualityPrefix}${formatHMin(remainMin)}` : `${qualityPrefix}—`;
    buildSegments(
      segmentsForMinutes(Math.max(0, remainMin), state.primaryWindowDurationMinutes),
      view,
    );
    return;
  }
  const pct = state.fiveHourPct;
  buildSegments(Math.round((SEGMENT_COUNT * (100 - pct)) / 100), view);
  view.element("autonomie").textContent = `${qualityPrefix}${Math.round(100 - pct)}%`;
}

/** Re-paints the "autonomie" row on each clock tick (D40). In official mode
 *  neither view changes on its own between pushes — this just re-asserts
 *  whichever one is toggled on so the tick never clobbers it. In estimated
 *  mode (no sensor ever connected) falls back to the time-until-reset
 *  countdown, unchanged from before D40: keeps counting even while
 *  `sensorConnected` is momentarily false — the known reset doesn't stop
 *  being valid just because the sensor is quiet for a while. */
export function refreshAutonomie(view = claudeView) {
  const state = view.state;
  if (state.everQuotaConnected) {
    paintQuotaGauge(view);
    return;
  }
  if (state.fiveHourResetsAtMs <= 0) return;
  const remainMin = (state.fiveHourResetsAtMs - Date.now()) / 60000;
  view.element("autonomie").textContent =
    remainMin > 0 ? formatHMin(remainMin) : "—";
}

/** Keeps Codex's locally observed thread duration moving between turn events. */
export function refreshSessionTime(view = claudeView) {
  if (view.provider !== "codex" || !(view.state.sessionStartedAtMs > 0)) return;
  view.element("session-time").textContent = formatDurationMs(
    Date.now() - view.state.sessionStartedAtMs,
  );
}

/** Click-toggle for the quota gauge (D40-toggle): flips between quota
 *  remaining and time-until-reset, both bar and text together (never a mix,
 *  D40). No-op in estimated mode — there's no quota to toggle to, so the
 *  row is left showing ccusage's time projection either way. */
export function toggleAutonomieView(view = claudeView) {
  const state = view.state;
  if (!state.everQuotaConnected) return;
  state.autonomieShowTime = !state.autonomieShowTime;
  paintQuotaGauge(view);
}

/** Paints the ccusage active block's data. odo/trip/avg always; the
 *  derived ones (segments/autonomie/gear) only if there is NO official sensor connected. */
export function onBlocksUpdate(block, view = claudeView) {
  const state = view.state;
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
  view.element("odo").textContent = formatTokens(tokens);

  const startedAt = block?.startTime ? Date.parse(block.startTime) : NaN;
  if (Number.isFinite(startedAt)) {
    view.element("session-time").textContent = formatDurationMs(
      Date.now() - startedAt
    );
  }

  const costUsd = Number(block?.costUsd) || 0;
  const perMtok = tokens > 0 ? (costUsd / tokens) * 1e6 : 0;
  view.element("avg").textContent = `$${perMtok.toFixed(2)}`;

  // Only if quota data NEVER arrived — once it did, a momentary pause
  // shouldn't let ccusage override the official data (D-review). Gated on
  // `everQuotaConnected`, not `everSensorConnected` (D40-fix): a non-Pro/Max
  // user's sensor connects but never carries quota, so ccusage's time
  // estimate must keep serving as the permanent fallback for them, not just
  // until the sensor's first (quota-less) payload.
  if (!state.everQuotaConnected) applyEstimated(block, view);
  // Lets Page 2 (limits-page.js) keep its instant/avg burn rate live while visible.
  view.emit("telemetry-tick");
}

/** OFFICIAL data from the statusLine: overwrites segments/autonomie/gear/warn. */
export function onSensorUpdate(p, view = claudeView) {
  const state = view.state;
  state.sensorConnected = true;
  state.everSensorConnected = true;
  state.rateLimitSourceQuality = "official";
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
    paintQuotaGauge(view);
  }
  if (p?.modelId) {
    setGear([p.modelId], view, {
      observedAtMs: p.observedAtMs,
      sequence: p.sequence ?? 0,
    });
  }
  // Official 7d rate-limit window — full numbers live on Page 2 (limits-page.js);
  // the border tint here stays as the always-visible "check engine"-style warning.
  const sevenDayFinite = Number.isFinite(p?.sevenDayPct);
  if (sevenDayFinite) {
    state.hasSecondaryLimit = true;
    state.sevenDayPct = Math.max(0, Math.min(100, p.sevenDayPct));
    state.sevenDayResetsAtMs = p?.sevenDayResetsAt
      ? Number(p.sevenDayResetsAt) * 1000
      : 0;
    view.root().classList.toggle("warn", state.sevenDayPct > 80);
  }
  // Sliding buffer for the footer's AUTO metric (see footer-metric.js).
  if (pctFinite) {
    state.recentPct.push({ recvAt: Date.now(), pct });
  }
  paintTurnContext(p, view);
  renderFooterMetric(view);
  view.emit("telemetry-tick");
}

/** Third column of Page 0: current-turn context window fill + prompt-cache
 *  hit rate. Fed by two different pipelines depending on provider — Claude's
 *  numbers arrive via the statusLine's official `context_window` (onSensorUpdate),
 *  Codex's via the rollout-derived `burn-tick` (onBurnTick, same source as tok/s).
 *  Only touches an element when its figure is present (D-review convention,
 *  see paintQuotaGauge): a tick from the OTHER pipeline (e.g. Claude's
 *  output-only burn-tick) must not blank out what the other one already painted. */
export function paintTurnContext(payload, view = claudeView) {
  const contextUsedPct = Number(payload?.contextUsedPct);
  if (Number.isFinite(contextUsedPct)) {
    view.element("context-left").textContent = `${Math.round(100 - contextUsedPct)}%`;
  }
  const cacheHitPct = Number(payload?.cacheHitPct);
  if (Number.isFinite(cacheHitPct)) {
    view.element("cache-hit").textContent = `${Math.round(cacheHitPct)}%`;
  }
}

/** Provider-neutral official rate-limit contract from Codex App Server. */
export function onRateLimitUpdate(p, view = claudeView) {
  const state = view.state;
  state.rateLimitSourceQuality = p?.sourceQuality ?? "unavailable";
  state.rateLimitBuckets = Array.isArray(p?.buckets) ? p.buckets : [];
  if (p?.sourceQuality !== "official") {
    state.sensorConnected = false;
    if (state.everQuotaConnected) paintQuotaGauge(view);
    renderFooterMetric(view);
    view.emit("telemetry-tick");
    return;
  }

  const primary = p?.primary ?? null;
  const secondary = p?.secondary ?? null;
  const windows = [primary, secondary].filter(Boolean);
  const duration = (window) =>
    Number.isFinite(window?.windowDurationMinutes)
      ? window.windowDurationMinutes
      : null;
  const shortWindow = windows
    .filter((window) => duration(window) != null && duration(window) < 24 * 60)
    .sort((left, right) => duration(left) - duration(right))[0];
  // `primary` is the account's main meter, but it is not necessarily a 5h
  // window. Plus currently returns a single 10080-minute primary window.
  const liveWindow = shortWindow ?? primary;
  const weeklyWindow =
    windows.find((window) => duration(window) === WEEKLY_WINDOW_MIN) ??
    windows
      .filter((window) => duration(window) != null && duration(window) >= 24 * 60)
      .sort(
        (left, right) =>
          Math.abs(duration(left) - WEEKLY_WINDOW_MIN) -
          Math.abs(duration(right) - WEEKLY_WINDOW_MIN),
      )[0] ??
    (secondary && duration(secondary) == null ? secondary : null);

  if (duration(liveWindow) != null) {
    state.primaryWindowDurationMinutes = duration(liveWindow);
  }
  state.secondaryWindowDurationMinutes = duration(weeklyWindow) != null
    ? duration(weeklyWindow)
    : null;
  state.hasSecondaryLimit = false;
  state.sevenDayPct = 0;
  state.sevenDayResetsAtMs = 0;
  onSensorUpdate(
    {
      observedAtMs: p?.observedAtMs,
      fiveHourPct: liveWindow?.usedPercent,
      fiveHourResetsAt: Number.isFinite(liveWindow?.resetsAtMs)
        ? liveWindow.resetsAtMs / 1000
        : null,
      sevenDayPct: weeklyWindow?.usedPercent,
      sevenDayResetsAt: Number.isFinite(weeklyWindow?.resetsAtMs)
        ? weeklyWindow.resetsAtMs / 1000
        : null,
    },
    view,
  );
}

function averageDailyTokens(dailyUsage, days = 7, nowMs = Date.now()) {
  const buckets = (Array.isArray(dailyUsage) ? dailyUsage : [])
    .map((bucket) => ({
      day: Date.parse(`${bucket?.startDate}T00:00:00Z`),
      tokens: Number(bucket?.tokens),
    }))
    .filter((bucket) => Number.isFinite(bucket.day) && bucket.tokens >= 0);
  if (buckets.length === 0) return null;
  const now = new Date(nowMs);
  const currentDay = Date.UTC(now.getUTCFullYear(), now.getUTCMonth(), now.getUTCDate());
  const firstDay = currentDay - (days - 1) * 86_400_000;
  const total = buckets
    .filter((bucket) => bucket.day >= firstDay && bucket.day <= currentDay)
    .reduce((sum, bucket) => sum + bucket.tokens, 0);
  return total / days;
}

export function renderCodexLiveUsage(view) {
  if (view.provider !== "codex") return;
  const usage = view.state.accountUsage;
  const available = usage && usage.sourceQuality !== "unavailable";
  const stale = usage?.sourceQuality === "stale";
  view.element("odo").textContent =
    available && Number.isFinite(usage.lifetimeTokens)
      ? formatTokens(usage.lifetimeTokens)
      : "—";
  view.element("odo-unit").textContent = stale ? "STALE tok" : "tok";
  view.element("avg-label").textContent = stale ? "STALE AVG 7D" : "AVG 7D";
  view.element("avg-unit").textContent = "tok/d";
  const dailyAverage = available ? averageDailyTokens(usage.dailyUsage) : null;
  view.element("avg").textContent = Number.isFinite(dailyAverage)
    ? formatTokens(dailyAverage)
    : "—";
}

export function onAccountUsageUpdate(p, view = claudeView) {
  view.state.accountUsage = p ?? null;
  renderCodexLiveUsage(view);
  view.emit("telemetry-tick");
}

/** Sensor connection. If official data NEVER arrived, falls back to the
 *  "EST" projection. If it already did, a momentary disconnection (normal
 *  idle, no Claude Code rendering) FREEZES as-is — jumping to ccusage's
 *  projection here is an independent 5h window system and the jump
 *  looked like an absurd number (e.g. official "0h17" → ccusage's
 *  "EST 4h31", found in review). */
export function onSensorState(p, view = claudeView) {
  const state = view.state;
  state.sensorConnected = !!p?.connected;
  if (state.sensorConnected) return;
  if (state.everQuotaConnected) state.rateLimitSourceQuality = "stale";
  if (state.everQuotaConnected) return; // frozen: don't touch anything
  view.root().classList.remove("warn");
  if (state.lastBlock) applyEstimated(state.lastBlock, view);
  else {
    view.element("autonomie").textContent = "EST —";
    buildSegments(0, view);
  }
}

/** Header-hint wiring for Page 0's static glyphs and numbers whose meaning
 *  isn't fully covered by their `.unit` label (D-review). The `.row.gauge`
 *  hint covers the fuel icon + segment bar + "3h12" text as one zone —
 *  they're one gauge, not three separate things to explain. `#burn`/`#avg`
 *  get one despite already having a unit label: D8/D11 are this project's
 *  most-documented points of confusion (tok/s isn't live, cost is estimated). */
export function wireTripComputerHints(view = claudeView) {
  const local = runtime(view);
  const labels = Object.fromEntries(modelSlots(view.provider).map((slot) => [slot.key, slot.label]));
  // Per-letter hints (not one static hintOnHover on the whole .gear): each
  // slot names its own model even when it isn't the active one, and the
  // active slot is called out explicitly. "custom" has no fixed identity
  // (D-review) so its label is resolved live from the last detected id.
  view.queryAll(".gear .g").forEach((el) => {
    const model = el.dataset.model;
    el.addEventListener("mouseenter", () => {
      const label =
        model === "custom" ? local.lastCustomLabel || labels.custom : labels[model];
      setHeaderHint(
        local.currentGearHit === model
          ? `active model: ${label}`
          : label,
      );
    });
    el.addEventListener("mouseleave", () => setHeaderHint(""));
  });
  const gaugeRow = view.query(".row.gauge");
  hintOnHover(gaugeRow, "primary limit remaining — click for reset time");
  gaugeRow.onclick = () => toggleAutonomieView(view);
  hintOnHover(view.element("burn"), "per-response rate");
  hintOnHover(
    view.element("avg"),
    view.provider === "codex"
      ? "average tokens per, current 7-day period"
      : "estimated cost, not official",
  );
  hintOnHover(
    view.element("odo"),
    view.provider === "codex"
      ? "account lifetime tokens"
      : "total tokens in the active interval",
  );
  hintOnHover(
    view.element("session-time"),
    view.provider === "codex" ? "current local thread elapsed" : "active interval elapsed",
  );
  hintOnHover(view.element("context-left"), "context window remaining, current turn");
  hintOnHover(view.element("cache-hit"), "prompt cache hit rate, current turn");
}
