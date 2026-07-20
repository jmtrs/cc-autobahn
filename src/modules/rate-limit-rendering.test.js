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

const [
  { createProviderState },
  { onAccountUsageUpdate, onRateLimitUpdate },
  { renderAccountUsage, renderWeeklyLimit },
] = await Promise.all([
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

test("primary-only 7-day Codex limit feeds Weekly and the primary gauge", () => {
  const view = fakeView();
  view.root().dataset = { providerAvailable: "true" };
  const snapshot = {
    provider: "codex",
    observedAtMs: 100,
    sourceQuality: "official",
    primary: {
      usedPercent: 54,
      windowDurationMinutes: 10_080,
      resetsAtMs: 2_000_000_000_000,
    },
    secondary: null,
    buckets: [],
  };

  onRateLimitUpdate(snapshot, view);
  renderWeeklyLimit(view);

  assert.equal(view.element("autonomie").textContent, "46%");
  assert.equal(view.state.primaryWindowDurationMinutes, 10_080);
  assert.equal(view.state.hasSecondaryLimit, true);
  assert.equal(view.element("limit-pct").textContent, "54%");
  assert.match(view.element("limit-reset").textContent, /^resets /);
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

test("unknown-duration secondary keeps legacy weekly role without becoming Live", () => {
  const view = fakeView();
  view.element("autonomie").textContent = "EST —";
  onRateLimitUpdate(
    {
      provider: "codex",
      observedAtMs: 300,
      sourceQuality: "official",
      primary: null,
      secondary: { usedPercent: 20, windowDurationMinutes: null, resetsAtMs: null },
      buckets: [],
    },
    view,
  );

  assert.equal(view.state.everQuotaConnected, false);
  assert.equal(view.state.hasSecondaryLimit, true);
  assert.equal(view.state.sevenDayPct, 20);
  assert.equal(view.element("autonomie").textContent, "EST —");
});

test("Codex account usage renders every official summary field", () => {
  const view = fakeView();
  view.state.accountUsage = {
    sourceQuality: "official",
    lifetimeTokens: 1_250_000,
    peakDailyTokens: 250_000,
    longestRunningTurnSeconds: 7_200,
    currentStreakDays: 3,
    longestStreakDays: 9,
  };
  renderAccountUsage(view);
  assert.equal(view.element("burn-left-label").textContent, "ACCOUNT · OFFICIAL");
  assert.match(view.element("burn-instant").textContent, /1\.25M tok · peak 250k/);
  assert.equal(view.element("burn-avg").textContent, "3/9d · 2:00");
});

test("unavailable Codex account usage clears stale values", () => {
  const view = fakeView();
  view.state.accountUsage = { sourceQuality: "unavailable", lifetimeTokens: 99 };
  renderAccountUsage(view);
  assert.equal(view.element("burn-left-label").textContent, "ACCOUNT · UNAVAILABLE");
  assert.equal(view.element("burn-instant").textContent, "—");
  assert.equal(view.element("burn-avg").textContent, "—");
});

test("Codex Live uses official lifetime tokens and a seven-calendar-day average", () => {
  const view = fakeView();
  const realNow = Date.now;
  Date.now = () => Date.parse("2026-07-20T12:00:00Z");
  try {
    onAccountUsageUpdate(
      {
        sourceQuality: "official",
        lifetimeTokens: 3_221_710_761,
        dailyUsage: [
          { startDate: "2026-07-14", tokens: 7_000_000 },
          { startDate: "2026-07-19", tokens: 14_000_000 },
        ],
      },
      view,
    );
  } finally {
    Date.now = realNow;
  }

  assert.equal(view.element("odo").textContent, "3.22G");
  assert.equal(view.element("avg-label").textContent, "AVG 7D");
  assert.equal(view.element("avg").textContent, "3.00M");
  assert.equal(view.element("avg-unit").textContent, "tok/d");
});

test("Codex Live anchors AVG 7D to today and qualifies stale account data", () => {
  const view = fakeView();
  const realNow = Date.now;
  Date.now = () => Date.parse("2026-07-20T12:00:00Z");
  try {
    onAccountUsageUpdate(
      {
        sourceQuality: "stale",
        lifetimeTokens: 10_000,
        dailyUsage: [{ startDate: "2026-06-01", tokens: 70_000_000 }],
      },
      view,
    );
  } finally {
    Date.now = realNow;
  }

  assert.equal(view.element("odo").textContent, "10k");
  assert.equal(view.element("odo-unit").textContent, "STALE tok");
  assert.equal(view.element("avg-label").textContent, "STALE AVG 7D");
  assert.equal(view.element("avg").textContent, "0");
});
