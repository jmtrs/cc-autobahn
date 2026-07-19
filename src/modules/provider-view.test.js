import assert from "node:assert/strict";
import test from "node:test";

import { createProviderView } from "./provider-view.js";
import { state } from "./telemetry-state.js";

function fakeDocument(roots) {
  const events = [];
  return {
    events,
    querySelector(selector) {
      return roots[selector] ?? null;
    },
    dispatchEvent(event) {
      events.push(event);
    },
  };
}

function fakeRoot(label) {
  return {
    querySelector: (selector) =>
      selector.startsWith("[data-provider-role") || selector.startsWith("[data-chassis-role")
        ? null
        : `${label}:${selector}`,
    querySelectorAll: (selector) => [`${label}:${selector}`],
  };
}

test("provider views isolate roots and state", () => {
  const claudeRoot = fakeRoot("claude");
  const codexRoot = fakeRoot("codex");
  const chassis = fakeRoot("chassis");
  const documentRoot = fakeDocument({
    '[data-provider-module="claude"]': claudeRoot,
    '[data-provider-module="codex"]': codexRoot,
    "[data-app-chassis]": chassis,
  });

  const claude = createProviderView({ provider: "claude", documentRoot });
  const codex = createProviderView({ provider: "codex", documentRoot });

  assert.strictEqual(claude.state, state.providers.claude);
  assert.strictEqual(codex.state, state.providers.codex);
  assert.equal(claude.element("burn"), "claude:#burn");
  assert.equal(codex.element("burn"), "codex:#burn");
  assert.equal(claude.chassisElement("nameplate"), "chassis:#nameplate");
});

test("provider view events retain their provider discriminator", () => {
  const documentRoot = fakeDocument({
    '[data-provider-module="codex"]': fakeRoot("codex"),
  });
  const view = createProviderView({ provider: "codex", documentRoot });

  view.emit("telemetry-tick");

  assert.equal(documentRoot.events[0].type, "telemetry-tick");
  assert.deepEqual(documentRoot.events[0].detail, { provider: "codex" });
});
