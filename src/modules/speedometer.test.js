import assert from "node:assert/strict";
import test from "node:test";

globalThis.localStorage = {
  getItem: () => null,
  setItem() {},
};

const { onBurnTick, startBurnAnimation } = await import("./speedometer.js");

function fakeView(provider) {
  const burn = { textContent: "", classList: { toggle() {} } };
  const root = { classList: { toggle() {} } };
  return {
    provider,
    state: { recentTicks: [] },
    root: () => root,
    element: () => burn,
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
      outputTokens: 75,
      elapsedMs: 3_000,
      tokensPerSecond: 25,
      partial: false,
    },
    codex,
  );

  assert.equal(codex.state.recentTicks.length, 1);
  assert.equal(codex.state.recentTicks[0].tokens, 75);
  assert.deepEqual(claude.state.recentTicks, []);
});
