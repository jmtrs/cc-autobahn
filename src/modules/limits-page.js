// Page 2 — official weekly rate-limit window + today's per-model cost split
// + instant vs. average burn rate. All three fields were already flowing
// into the frontend (sevenDayPct/sevenDayResetsAt from the statusline
// sensor, D23; burnRate.costPerHour from blocks-update) but were either
// reduced to a border tint or never painted at all (D-review). This page
// is where they finally get real numbers, without crowding Page 0.

import { formatDurationMs, formatModelCode, formatResetAt, formatTokens, formatUsd } from "./format.js";
import { hintOnHover } from "./header-hint.js";
import {
  formatHistoryCost,
  loadHistory,
  SPINNER_HTML,
  todayEntry,
} from "./history-data.js";
import { claudeView } from "./provider-view.js";

const LIMIT_SEGMENTS = 12;
const wiringByProvider = new Map();

function paintWeeklyBar(pct, view) {
  const bar = view.element("limit-bar");
  bar.innerHTML = "";
  const filled = Math.max(0, Math.min(LIMIT_SEGMENTS, Math.round((LIMIT_SEGMENTS * pct) / 100)));
  for (let i = 0; i < LIMIT_SEGMENTS; i++) {
    const seg = document.createElement("div");
    seg.className = i < filled ? "seg on" : "seg";
    bar.appendChild(seg);
  }
}

export function renderWeeklyLimit(view) {
  const state = view.state;
  if (view.root().dataset.providerAvailable === "false") {
    view.element("limit-pct").textContent = "—";
    paintWeeklyBar(0, view);
    view.element("limit-reset").textContent = "data source unavailable";
    return;
  }
  if (view.provider === "codex" && state.rateLimitSourceQuality === "unavailable") {
    view.element("limit-pct").textContent = "—";
    paintWeeklyBar(0, view);
    view.element("limit-reset").textContent = "data source unavailable";
    return;
  }
  const hasData = state.hasSecondaryLimit || state.sevenDayResetsAtMs > 0;
  const pct = Number(state.sevenDayPct) || 0;
  view.element("limit-pct").textContent = hasData ? `${Math.round(pct)}%` : "—";
  paintWeeklyBar(pct, view);
  const quality = state.rateLimitSourceQuality === "stale" ? "stale · " : "";
  view.element("limit-reset").textContent = hasData
    ? state.sevenDayResetsAtMs > 0
      ? `${quality}resets ${formatResetAt(state.sevenDayResetsAtMs)}`
      : `${quality}reset unavailable`
    : "no official data yet";
}

function renderBurnRates(view) {
  if (view.provider === "codex") {
    renderAccountUsage(view);
    return;
  }
  view.element("burn-left-label").textContent = "INSTANT";
  view.element("burn-right-label").textContent = "AVG";
  const state = view.state;
  const block = state.lastBlock;
  const instant = Number(block?.burnRate?.costPerHour) || 0;
  const startedAt = block?.startTime ? Date.parse(block.startTime) : NaN;
  const elapsedHr = Number.isFinite(startedAt)
    ? Math.max((Date.now() - startedAt) / 3_600_000, 1 / 60)
    : 0;
  const avg = elapsedHr > 0 ? (Number(block?.costUsd) || 0) / elapsedHr : 0;
  view.element("burn-instant").textContent = block ? `${formatUsd(instant)}/h` : "—";
  view.element("burn-avg").textContent = block ? `${formatUsd(avg)}/h` : "—";
}

export function renderAccountUsage(view) {
  const usage = view.state.accountUsage;
  const quality = usage?.sourceQuality ?? "unavailable";
  view.element("burn-left-label").textContent = `ACCOUNT · ${quality.toUpperCase()}`;
  view.element("burn-right-label").textContent = "STREAK · TURN";
  if (!usage || quality === "unavailable") {
    view.element("burn-instant").textContent = "—";
    view.element("burn-avg").textContent = "—";
    return;
  }
  const lifetime = Number.isFinite(usage.lifetimeTokens)
    ? `${formatTokens(usage.lifetimeTokens)} tok`
    : "—";
  const peak = Number.isFinite(usage.peakDailyTokens)
    ? `peak ${formatTokens(usage.peakDailyTokens)}`
    : "peak —";
  view.element("burn-instant").textContent = `${lifetime} · ${peak}`;
  const current = Number.isFinite(usage.currentStreakDays) ? usage.currentStreakDays : "—";
  const longest = Number.isFinite(usage.longestStreakDays) ? usage.longestStreakDays : "—";
  const turn = Number.isFinite(usage.longestRunningTurnSeconds)
    ? formatDurationMs(usage.longestRunningTurnSeconds * 1000)
    : "—";
  view.element("burn-avg").textContent = `${current}/${longest}d · ${turn}`;
}

async function renderBreakdown(view, isMounted) {
  const list = view.element("breakdown-list");
  const nativeHistoryAvailable =
    typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
  if (view.root().dataset.providerAvailable === "false" && !nativeHistoryAvailable) {
    list.innerHTML = `<div class="ghost">data source unavailable</div>`;
    return;
  }
  // Same cold-load gap as History (D-review): on a first, uncached
  // loadHistory() call this list otherwise stays empty/stale with no
  // indication anything is happening.
  list.innerHTML = SPINNER_HTML;
  try {
    const days = await loadHistory(view.provider);
    if (!isMounted()) return;
    const today = todayEntry(days);
    const models = today?.modelBreakdowns ?? [];
    if (models.length === 0) {
      list.innerHTML = `<div class="ghost">no usage today</div>`;
      return;
    }
    list.innerHTML = models
      .slice()
      .sort((a, b) => (b.cost || 0) - (a.cost || 0))
      // Single-letter code (model-chip) instead of the full model id — a long
      // id like "claude-haiku-4-5-20251001" would truncate in this column.
      .map(
        (m) =>
          `<div class="breakdown-row"><span class="model-chip"><span class="code">${formatModelCode(m.modelName, view.provider)}</span></span><span>${formatHistoryCost(m.cost, view.provider)}</span></div>`
      )
      .join("");
    renderWeeklyLimit(view);
  } catch (e) {
    if (isMounted()) list.innerHTML = `<div class="ghost">no data</div>`;
    console.error(`[limits:${view.provider}] history_daily:`, e);
  }
}

function isPageActive(view) {
  return view.element("page-2")?.classList.contains("active");
}

export function wireLimitsPage(view = claudeView) {
  if (wiringByProvider.has(view.provider)) return wiringByProvider.get(view.provider);
  // Whole column (%, bar, reset time) as one hint zone — same reasoning as
  // Page 0's .row.gauge, it's one gauge, not three things to explain.
  hintOnHover(
    view.query(".limits-col"),
    view.provider === "codex"
      ? "Official Codex secondary usage window"
      : "Official 7-day usage window, resets weekly"
  );
  hintOnHover(view.element("breakdown-list"), "Cost by model, today");
  hintOnHover(
    view.query(".burn-rates"),
    view.provider === "codex"
      ? "Official account lifetime, peak, streak and longest turn"
      : "Instant vs. this block's average $/h"
  );
  let mounted = true;
  const onPageChanged = (e) => {
    if (e.detail.page !== 2) return;
    renderWeeklyLimit(view);
    renderBurnRates(view);
    renderBreakdown(view, () => mounted);
  };
  // Keep the two cheap fields live while the page is on screen — the
  // breakdown (today's cost split) doesn't need second-by-second refresh.
  const onTelemetryTick = (e) => {
    if (e.detail?.provider !== view.provider || !isPageActive(view)) return;
    renderWeeklyLimit(view);
    renderBurnRates(view);
  };
  document.addEventListener("mfd-page-changed", onPageChanged);
  document.addEventListener("telemetry-tick", onTelemetryTick);
  const dispose = () => {
    if (!mounted) return;
    mounted = false;
    document.removeEventListener("mfd-page-changed", onPageChanged);
    document.removeEventListener("telemetry-tick", onTelemetryTick);
    if (wiringByProvider.get(view.provider) === dispose) {
      wiringByProvider.delete(view.provider);
    }
  };
  wiringByProvider.set(view.provider, dispose);
  return dispose;
}
