import assert from "node:assert/strict";
import test from "node:test";

function fakeElement() {
  return {
    addEventListener() {},
    removeEventListener() {},
  };
}

function fakeDocument() {
  const listeners = new Map();
  return {
    querySelector: () => null,
    addEventListener(name, listener) {
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(listener);
    },
    removeEventListener(name, listener) {
      listeners.get(name)?.delete(listener);
    },
    listenerCount(name) {
      return listeners.get(name)?.size ?? 0;
    },
    dispatch(name, detail) {
      listeners.get(name)?.forEach((listener) => listener({ detail }));
    },
  };
}

globalThis.document = fakeDocument();
globalThis.window = {};

const [{ wireHistoryPage }, { wireLimitsPage }] = await Promise.all([
  import("./history-page.js"),
  import("./limits-page.js"),
]);

function fakeView(provider) {
  return {
    provider,
    state: {},
    root: () => ({ dataset: { providerAvailable: "true" } }),
    element: () => fakeElement(),
    query: () => fakeElement(),
  };
}

test("provider page wiring is idempotent and disposable", () => {
  const historyView = fakeView("codex");
  const disposeHistory = wireHistoryPage(historyView);
  assert.strictEqual(wireHistoryPage(historyView), disposeHistory);
  assert.equal(document.listenerCount("mfd-page-changed"), 1);
  disposeHistory();
  assert.equal(document.listenerCount("mfd-page-changed"), 0);

  const limitsView = fakeView("codex");
  const disposeLimits = wireLimitsPage(limitsView);
  assert.strictEqual(wireLimitsPage(limitsView), disposeLimits);
  assert.equal(document.listenerCount("mfd-page-changed"), 1);
  assert.equal(document.listenerCount("telemetry-tick"), 1);
  disposeLimits();
  assert.equal(document.listenerCount("mfd-page-changed"), 0);
  assert.equal(document.listenerCount("telemetry-tick"), 0);
});

test("disposed history wiring ignores an in-flight refresh result", async () => {
  let elementReads = 0;
  const view = {
    ...fakeView("claude"),
    element() {
      elementReads += 1;
      return fakeElement();
    },
  };
  const dispose = wireHistoryPage(view);

  document.dispatch("mfd-page-changed", { page: 1 });
  const readsBeforeDispose = elementReads;
  dispose();
  await Promise.resolve();
  await Promise.resolve();

  assert.equal(elementReads, readsBeforeDispose);
});
