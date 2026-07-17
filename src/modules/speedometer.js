// Speedometer — physical spring (D8).
// The needle jumps to the tok/s of the completed turn and decays with a spring to idle.
// This is the HONEST reading: "tok/s per response", never "instantaneous" (D11).

import { formatTps } from "./format.js";
import { renderFooterMetric } from "./footer-metric.js";
import { state } from "./telemetry-state.js";

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

/** Spring animation loop. Always runs (also to decay). */
export function burnFrame(now) {
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
export function onBurnTick(payload) {
  // payload = { tokPerS, turnOutputTokens, turnDurationMs, messageId, timestamp, isPartial }
  const tps = Number(payload?.tokPerS) || 0;
  burn.target = tps;
  burn.lastTickAt = performance.now();
  // Sliding buffer for the footer's PACE metric (see footer-metric.js).
  // Only final (non-partial) ticks are counted: a partial tick's tokens are
  // already re-included in the aggregate of the turn-closing tick (D27), so
  // pushing both would double-count them.
  const tokens = Number(payload?.turnOutputTokens) || 0;
  if (tokens > 0 && !payload?.isPartial) {
    state.recentTicks.push({ recvAt: Date.now(), tokens });
  }
  renderFooterMetric();
}
