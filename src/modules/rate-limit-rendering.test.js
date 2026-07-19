import assert from "node:assert/strict";
import test from "node:test";

globalThis.localStorage = {
  getItem: () => null,
  setItem() {},
};
globalThis.document = {
  createElement() {
    return { className: "" };
  },
};

const [{ createProviderState }, { onRateLimitUpdate }, { renderWeeklyLimit }] = await Promise.all([
  import("./telemetry-state.js"),
  import("./trip-computer.js"),
  import("./limits-page.js"),
]);

function fakeElement() {
  let html = "";
  const element = {
    textContent: "",
    children: [],
    classList: { toggle() {}, remove() {} },
    appendChild(child) {
      this.children.push(child);
    },
  };
  Object.defineProperty(element, "innerHTML", {
    get: () => html,
    set(value) {
      html = value;
      element.children = [];
    },
  });
  return element;
}

function fakeView() {
  const elements = new Map();
  const root = fakeElement();
  return {
    provider: "codex",
    state: createProviderState("codex"),
    root: () => root,
    element(role) {
      if (!elements.has(role)) elements.set(role, fakeElement());
      return elements.get(role);
    },
    emit() {},
  };
}

test("Codex Live gauge distinguishes official, stale and unavailable limits", () => {
  const view = fakeView();
  const snapshot = {
    provider: "codex",
    observedAtMs: 100,
    sourceQuality: "official",
    primary: {
      usedPercent: 25,
      windowDurationMinutes: 300,
      resetsAtMs: 2_000_000_000_000,
    },
    secondary: null,
    buckets: [],
  };

  onRateLimitUpdate(snapshot, view);
  assert.equal(view.element("autonomie").textContent, "75%");

  onRateLimitUpdate({ ...snapshot, sourceQuality: "stale" }, view);
  assert.equal(view.element("autonomie").textContent, "STALE 75%");

  onRateLimitUpdate({ ...snapshot, sourceQuality: "unavailable" }, view);
  assert.equal(view.element("autonomie").textContent, "UNAVAILABLE");
});

test("Codex Limits page does not present unavailable App Server data as current", () => {
  const view = fakeView();
  view.state.rateLimitSourceQuality = "unavailable";
  view.state.hasSecondaryLimit = true;
  view.state.sevenDayPct = 81;
  view.state.sevenDayResetsAtMs = 2_000_000_000_000;
  view.root().dataset = { providerAvailable: "true" };

  renderWeeklyLimit(view);

  assert.equal(view.element("limit-pct").textContent, "—");
  assert.equal(view.element("limit-reset").textContent, "data source unavailable");
});

test("secondary-only stale update does not invent a primary 100% gauge", () => {
  const view = fakeView();
  view.element("autonomie").textContent = "EST —";
  const snapshot = {
    provider: "codex",
    observedAtMs: 200,
    sourceQuality: "official",
    primary: null,
    secondary: { usedPercent: 20, windowDurationMinutes: 10080, resetsAtMs: null },
    buckets: [],
  };

  onRateLimitUpdate(snapshot, view);
  onRateLimitUpdate({ ...snapshot, sourceQuality: "stale" }, view);

  assert.equal(view.state.everQuotaConnected, false);
  assert.equal(view.element("autonomie").textContent, "EST —");
});
