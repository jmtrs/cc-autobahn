import test from "node:test";
import assert from "node:assert/strict";

import { hydrateProviderHealth, routeClaudePayload } from "./provider-routing.js";
import { state, updateProviderHealth } from "./telemetry-state.js";

test("IPC routing invokes legacy renderer only for Claude payloads", () => {
  const seen = [];
  assert.equal(
    routeClaudePayload({ provider: "claude", value: 1 }, (payload) => seen.push(payload.value)),
    true
  );
  assert.equal(
    routeClaudePayload({ provider: "codex", value: 2 }, (payload) => seen.push(payload.value)),
    false
  );
  assert.equal(routeClaudePayload({ value: 3 }, (payload) => seen.push(payload.value)), false);
  assert.deepEqual(seen, [1]);
});

test("channel routing rejects a delayed sensor snapshot after a newer event", () => {
  state.providers.claude.lastEventAtMs = {};
  const seen = [];
  assert.equal(
    routeClaudePayload(
      { provider: "claude", observedAtMs: 200, value: "event" },
      (payload) => seen.push(payload.value),
      "sensor-update"
    ),
    true
  );
  assert.equal(
    routeClaudePayload(
      { provider: "claude", observedAtMs: 199, value: "snapshot" },
      (payload) => seen.push(payload.value),
      "sensor-update"
    ),
    false
  );
  assert.equal(
    routeClaudePayload(
      { provider: "claude", observedAtMs: 200, value: "equal snapshot" },
      (payload) => seen.push(payload.value),
      "sensor-update",
      true
    ),
    false
  );
  assert.deepEqual(seen, ["event"]);
});

test("health snapshot hydrates missed startup events without replaying older state", () => {
  state.providers.claude.health = {};
  state.providers.codex.health = {};
  updateProviderHealth({
    provider: "codex",
    component: "sensor",
    status: "connected",
    observedAtMs: 200,
  });

  const applied = hydrateProviderHealth([
    {
      provider: "claude",
      component: "engine",
      status: "connected",
      observedAtMs: 100,
    },
    {
      provider: "codex",
      component: "sensor",
      status: "degraded",
      observedAtMs: 199,
    },
  ]);

  assert.equal(applied, 1);
  assert.equal(state.providers.claude.health.engine.status, "connected");
  assert.equal(state.providers.codex.health.sensor.status, "connected");
});
