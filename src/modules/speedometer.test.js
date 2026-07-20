import assert from "node:assert/strict";
import test from "node:test";

globalThis.localStorage = {
  getItem: () => null,
  setItem() {},
};

const { onBurnTick, startBurnAnimation } = await import("./speedometer.js");

function fakeView(provider) {
  const elements = new Map();
  const root = { classList: { toggle() {} } };
  return {
    provider,
    state: { recentTicks: [] },
    root: () => root,
    element(role) {
      if (!elements.has(role)) {
        elements.set(role, { textContent: "", classList: { toggle() {} }, querySelectorAll: () => [] });
      }
      return elements.get(role);
    },
  };
}

test("burn animation is idempotent and restartable per provider", () => {
  const callbacks = [];
  const cancelled = [];
  let nextId = 0;
  const requestFrame = (callback) => {
    callbacks.push(callback);
    nextId += 1;
    return nextId;
  };
  const cancelFrame = (id) => cancelled.push(id);
  const view = fakeView("codex");

  const stopFirst = startBurnAnimation(view, requestFrame, cancelFrame);
  const stopDuplicate = startBurnAnimation(view, requestFrame, cancelFrame);

  assert.strictEqual(stopDuplicate, stopFirst);
  assert.equal(callbacks.length, 1);

  callbacks[0](0);
  assert.equal(callbacks.length, 2);
  stopFirst();
  assert.deepEqual(cancelled, [2]);

  startBurnAnimation(view, requestFrame, cancelFrame);
  assert.equal(callbacks.length, 3);
});

test("normalized Codex turn rates feed only their provider buffer", () => {
  const codex = fakeView("codex");
  const claude = fakeView("claude");

  onBurnTick(
    {
      provider: "codex",
      sessionOrThreadId: "thread-1",
      sessionStartedAtMs: 1_000,
      observedAtMs: 2_000,
      outputTokens: 75,
      elapsedMs: 3_000,
      tokensPerSecond: 25,
      partial: false,
      sourceQuality: "local",
    },
    codex,
  );

  assert.equal(codex.state.recentTicks.length, 1);
  assert.equal(codex.state.recentTicks[0].tokens, 75);
  assert.equal(codex.state.turnRateSourceQuality, "local");
  assert.equal(codex.state.activeSessionOrThreadId, "thread-1");
  assert.equal(codex.state.sessionStartedAtMs, 1_000);
  assert.equal(codex.element("burn-unit").textContent, "LOCAL tok/s");
  assert.deepEqual(claude.state.recentTicks, []);
});

test("older Codex turn rates cannot roll the live thread timer backwards", () => {
  const codex = fakeView("codex");
  onBurnTick(
    {
      sessionOrThreadId: "newer",
      sessionStartedAtMs: 2_000,
      observedAtMs: 5_000,
    },
    codex,
  );
  onBurnTick(
    {
      sessionOrThreadId: "older",
      sessionStartedAtMs: 1_000,
      observedAtMs: 4_000,
    },
    codex,
  );

  assert.equal(codex.state.activeSessionOrThreadId, "newer");
  assert.equal(codex.state.sessionStartedAtMs, 2_000);
});

test("legacy Claude rate keeps the unqualified unit", () => {
  const claude = fakeView("claude");
  onBurnTick({ tokPerS: 10, turnOutputTokens: 5, isPartial: false }, claude);
  assert.equal(claude.element("burn-unit").textContent, "tok/s");
});
