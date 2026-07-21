// Speedometer — physical spring (D8).
// The needle jumps to the tok/s of the completed turn and decays with a spring to idle.
// This is the HONEST reading: "tok/s per response", never "instantaneous" (D11).

import { formatTps } from "./format.js";
import { renderFooterMetric } from "./footer-metric.js";
import { paintTurnContext } from "./trip-computer.js";
import { claudeView } from "./provider-view.js";

const burns = new Map();

function burnFor(view) {
  if (!burns.has(view.provider)) {
    burns.set(view.provider, {
      target: 0,
      pos: 0,
      vel: 0,
      lastTickAt: 0,
      SPRING_K: 0.2,
      SPRING_DAMP: 0.75,
      IDLE_AFTER_MS: 2000,
      IDLE_DECAY: 0.95,
      animation: null,
    });
  }
  return burns.get(view.provider);
}

/** Advances one spring frame. Scheduling is owned by startBurnAnimation(). */
export function burnFrame(now, view = claudeView) {
  const burn = burnFor(view);
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

  view.element("burn").textContent = formatTps(burn.pos);
}

/** Starts at most one animation loop per provider and returns its disposer. */
export function startBurnAnimation(
  view = claudeView,
  requestFrame = requestAnimationFrame,
  cancelFrame = cancelAnimationFrame,
) {
  const burn = burnFor(view);
  if (burn.animation) return burn.animation.stop;

  const animation = { frameId: null, running: true, stop: null };
  const frame = (now) => {
    if (!animation.running) return;
    burnFrame(now, view);
    animation.frameId = requestFrame(frame);
  };
  animation.stop = () => {
    if (!animation.running) return;
    animation.running = false;
    if (animation.frameId != null) cancelFrame(animation.frameId);
    burn.animation = null;
  };
  burn.animation = animation;
  animation.frameId = requestFrame(frame);
  return animation.stop;
}

/** Handles a burn-tick from the backend (closed turn or intermediate message, D27). */
export function onBurnTick(payload, view = claudeView) {
  const burn = burnFor(view);
  const state = view.state;
  // payload = { tokPerS, turnOutputTokens, turnDurationMs, messageId, timestamp, isPartial }
  const tps = Number(payload?.tokensPerSecond ?? payload?.tokPerS) || 0;
  if (
    view.provider === "codex" &&
    typeof payload?.sessionOrThreadId === "string" &&
    Number.isFinite(payload?.sessionStartedAtMs) &&
    Number.isFinite(payload?.observedAtMs) &&
    payload.observedAtMs > (Number(state.lastTurnRateObservedAtMs) || 0)
  ) {
    state.activeSessionOrThreadId = payload.sessionOrThreadId;
    state.sessionStartedAtMs = payload.sessionStartedAtMs;
    state.lastTurnRateObservedAtMs = payload.observedAtMs;
  }
  burn.target = tps;
  burn.lastTickAt = performance.now();
  // Sliding buffer for the footer's PACE metric (see footer-metric.js).
  // Only final (non-partial) ticks are counted: a partial tick's tokens are
  // already re-included in the aggregate of the turn-closing tick (D27), so
  // pushing both would double-count them.
  const tokens = Number(payload?.outputTokens ?? payload?.turnOutputTokens) || 0;
  const partial = payload?.partial ?? payload?.isPartial;
  if (tokens > 0 && !partial) {
    state.recentTicks.push({ recvAt: Date.now(), tokens });
  }
  // Codex's burn-tick carries context/cache figures piggybacked from the same
  // rollout token_count read (see trip-computer.js). Claude's doesn't — a
  // no-op there, official numbers keep arriving via onSensorUpdate instead.
  paintTurnContext(payload, view);
  renderFooterMetric(view);
}
