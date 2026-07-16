// Footer: PACE (recent pace vs. block average) / AUTO (autonomy adjusted to
// the recent pace, official sensor only). Replaces the old "LAST tok/s"
// (D27 made it ambiguous: full turn vs. intermediate message).
// Verified against ccusage's actual source code (D28): burnRate.tokensPerMinute
// = block's totalTokens/minutes (the same calculation we would do, so it's
// reused); projection.remainingMinutes is pure clock, it doesn't depend on
// pace — that's why AUTO only makes sense with the official sensor
// (rate_limits does measure real quota consumption).

import { formatHMin } from "./format.js";
import { state } from "./telemetry-state.js";

const PACE_WINDOW_MS = 5 * 60 * 1000; // recent window for PACE
const PACE_MIN_BLOCK_ELAPSED_MIN = 1; // minimum block elapsed before trusting blockAvg
const PACE_MIN_SPAN_MIN = 0.5; // minimum tick span before trusting recentRate
const AUTONOMY_WINDOW_MS = 10 * 60 * 1000; // recent window for AUTO
const AUTONOMY_MIN_SPAN_MIN = 2; // minimum real span before trusting the pace

let footerMetric =
  localStorage.getItem("cc-autobahn.footerMetric") === "autonomy"
    ? "autonomy"
    : "pace";

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
  state.recentTicks = state.recentTicks.filter((t) => now - t.recvAt <= PACE_WINDOW_MS);
  const outputTokens = Number(state.lastBlock?.tokenCounts?.outputTokens);
  const startedAt = state.lastBlock?.startTime ? Date.parse(state.lastBlock.startTime) : NaN;
  if (
    !Number.isFinite(outputTokens) ||
    !Number.isFinite(startedAt) ||
    state.recentTicks.length === 0
  ) {
    return null;
  }
  // Block just started: dividing by an elapsed time close to zero inflates
  // blockAvg artificially (same kind of noise already avoided in AUTO).
  const elapsedMin = (now - startedAt) / 60000;
  if (elapsedMin < PACE_MIN_BLOCK_ELAPSED_MIN) return null;
  const blockAvg = outputTokens / elapsedMin;
  if (blockAvg <= 0) return null;

  const totalTokens = state.recentTicks.reduce((sum, t) => sum + t.tokens, 0);
  const spanMin = (now - state.recentTicks[0].recvAt) / 60000;
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
  if (!state.sensorConnected) return null;
  const now = Date.now();
  state.recentPct = state.recentPct.filter((p) => now - p.recvAt <= AUTONOMY_WINDOW_MS);
  if (state.recentPct.length < 2) return null;
  const oldest = state.recentPct[0];
  const newest = state.recentPct[state.recentPct.length - 1];
  const spanMin = (newest.recvAt - oldest.recvAt) / 60000;
  if (spanMin < AUTONOMY_MIN_SPAN_MIN) return null;
  const ratePerMin = (newest.pct - oldest.pct) / spanMin;
  if (ratePerMin <= 0) return null;
  let minutesLeft = (100 - newest.pct) / ratePerMin;

  if (state.fiveHourResetsAtMs > 0) {
    const wallClockRemainingMin = (state.fiveHourResetsAtMs - now) / 60000;
    if (wallClockRemainingMin <= 0) return null; // reset should already have happened
    minutesLeft = Math.min(minutesLeft, wallClockRemainingMin);
  }
  return minutesLeft;
}

/** Repaints the footer based on the active metric (PACE/AUTO, see footerMetric). */
export function renderFooterMetric() {
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
export function wireFooterToggle() {
  document.getElementById("footer-metric").onclick = () => {
    footerMetric = footerMetric === "pace" ? "autonomy" : "pace";
    localStorage.setItem("cc-autobahn.footerMetric", footerMetric);
    renderFooterMetric();
  };
}
