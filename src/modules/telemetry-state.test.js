import test from "node:test";
import assert from "node:assert/strict";

import {
  claudeState,
  createProviderState,
  providerIdFromPayload,
  setCurrentPage,
  setPermissionHead,
  state,
  updateProviderHealth,
} from "./telemetry-state.js";

test("Claude and Codex provider state never share mutable buffers", () => {
  const claude = createProviderState("claude");
  const codex = createProviderState("codex");
  claude.recentTicks.push({ tokens: 10 });
  claude.health.sensor = { status: "connected" };

  assert.deepEqual(codex.recentTicks, []);
  assert.deepEqual(codex.health, {});
  assert.notStrictEqual(claude.recentTicks, codex.recentTicks);
  assert.notStrictEqual(claude.health, codex.health);
});

test("legacy renderers are explicitly bound to Claude state", () => {
  assert.strictEqual(claudeState, state.providers.claude);
  assert.notStrictEqual(claudeState, state.providers.codex);
});

test("payload routing rejects missing and unknown provider discriminants", () => {
  assert.equal(providerIdFromPayload({ provider: "claude" }), "claude");
  assert.equal(providerIdFromPayload({ provider: "codex" }), "codex");
  assert.equal(providerIdFromPayload({}), null);
  assert.equal(providerIdFromPayload({ provider: "other" }), null);
});

test("component health updates only its provider", () => {
  state.providers.claude.health = {};
  state.providers.codex.health = {};

  assert.equal(
    updateProviderHealth({
      provider: "codex",
      component: "app-server",
      status: "degraded",
      observedAtMs: 42,
      detail: "method unavailable",
    }),
    true
  );
  assert.deepEqual(state.providers.claude.health, {});
  assert.deepEqual(state.providers.codex.health["app-server"], {
    status: "degraded",
    observedAtMs: 42,
    detail: "method unavailable",
  });
});

test("older or invalid health cannot replace a current component snapshot", () => {
  state.providers.codex.health = {};
  assert.equal(
    updateProviderHealth({
      provider: "codex",
      component: "sensor",
      status: "connected",
      observedAtMs: 100,
    }),
    true
  );
  assert.equal(
    updateProviderHealth({
      provider: "codex",
      component: "sensor",
      status: "degraded",
      observedAtMs: 99,
    }),
    false
  );
  assert.equal(
    updateProviderHealth({
      provider: "codex",
      component: "unknown",
      status: "connected",
      observedAtMs: 101,
    }),
    false
  );
  assert.equal(state.providers.codex.health.sensor.status, "connected");
});

test("shared chassis page and permission head update independently of provider telemetry", () => {
  assert.equal(setCurrentPage(2), true);
  assert.equal(setCurrentPage(9), false);
  assert.equal(state.global.currentPage, 2);

  const head = { provider: "claude", id: "request-1" };
  setPermissionHead(head);
  assert.strictEqual(state.global.permissionHead, head);
  setPermissionHead(null);
  assert.equal(state.global.permissionHead, null);
});
