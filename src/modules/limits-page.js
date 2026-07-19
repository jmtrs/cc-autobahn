// Page 2 — official weekly rate-limit window + today's per-model cost split
// + instant vs. average burn rate. All three fields were already flowing
// into the frontend (sevenDayPct/sevenDayResetsAt from the statusline
// sensor, D23; burnRate.costPerHour from blocks-update) but were either
// reduced to a border tint or never painted at all (D-review). This page
// is where they finally get real numbers, without crowding Page 0.

import { formatModelCode, formatResetAt, formatUsd } from "./format.js";
import { hintOnHover } from "./header-hint.js";
import { latestDay, loadHistory, SPINNER_HTML } from "./history-data.js";
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

function renderWeeklyLimit(view) {
  const state = view.state;
  if (view.root().dataset.providerAvailable === "false") {
    view.element("limit-pct").textContent = "—";
    paintWeeklyBar(0, view);
    view.element("limit-reset").textContent = "data source unavailable";
    return;
  }
  const hasData = state.sevenDayResetsAtMs > 0;
  const pct = Number(state.sevenDayPct) || 0;
  view.element("limit-pct").textContent = hasData ? `${Math.round(pct)}%` : "—";
  paintWeeklyBar(pct, view);
  view.element("limit-reset").textContent = hasData
    ? `resets ${formatResetAt(state.sevenDayResetsAtMs)}`
    : "no official data yet";
}

function renderBurnRates(view) {
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

async function renderBreakdown(view, isMounted) {
  const list = view.element("breakdown-list");
  if (view.root().dataset.providerAvailable === "false" || view.provider !== "claude") {
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
    const today = latestDay(days);
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
          `<div class="breakdown-row"><span class="model-chip"><span class="code">${formatModelCode(m.modelName)}</span></span><span>${formatUsd(m.cost)}</span></div>`
      )
      .join("");
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
    "Official 7-day usage window, resets weekly"
  );
  hintOnHover(view.element("breakdown-list"), "Cost by model, today");
  hintOnHover(view.query(".burn-rates"), "Instant vs. this block's average $/h");
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
