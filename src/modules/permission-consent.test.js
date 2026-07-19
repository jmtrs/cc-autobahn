import assert from "node:assert/strict";
import test from "node:test";

import { permissionStatusLabel } from "./permission-consent.js";

test("Codex permission status distinguishes lifecycle states", () => {
  assert.equal(permissionStatusLabel("codex", null), "Connect");
  assert.equal(permissionStatusLabel("codex", { installed: true }), "Disconnected");
  assert.equal(
    permissionStatusLabel("codex", { configuredLocally: true }),
    "Disconnected",
  );
  assert.equal(
    permissionStatusLabel("codex", {
      installed: true,
      enabled: true,
      trustStatus: "untrusted",
    }),
    "Await trust",
  );
  assert.equal(
    permissionStatusLabel("codex", {
      installed: true,
      enabled: true,
      trustStatus: "modified",
      active: true,
    }),
    "Await trust",
  );
  assert.equal(
    permissionStatusLabel("codex", {
      installed: true,
      enabled: true,
      trustStatus: "trusted",
    }),
    "Ready",
  );
  assert.equal(
    permissionStatusLabel("codex", {
      installed: true,
      enabled: true,
      trustStatus: "trusted",
      active: true,
    }),
    "Active",
  );
});

test("Claude permission status keeps its existing manage state", () => {
  assert.equal(permissionStatusLabel("claude", { installed: true }), "Manage");
});
