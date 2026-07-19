import assert from "node:assert/strict";
import test from "node:test";

import { setProviderAvailability, setProviderIssue } from "./provider-status.js";

function providerRoot(provider, available = true) {
  const label = { textContent: "" };
  return {
    dataset: {
      providerModule: provider,
      providerAvailable: String(available),
    },
    label,
    setAttribute(name, value) {
      this[name] = value;
    },
    querySelector: () => label,
  };
}

test("provider issues remain local and compose without hiding identity", () => {
  const claude = providerRoot("claude");
  const codex = providerRoot("codex", false);
  const roots = { claude, codex };
  const documentRoot = {
    querySelector(selector) {
      return roots[selector.includes("claude") ? "claude" : "codex"];
    },
  };

  setProviderIssue("claude", "engine", "CHECK ENGINE", true, documentRoot);
  setProviderIssue("claude", "sensor", "SENSOR OFFLINE", true, documentRoot);

  assert.equal(claude.label.textContent, "CLAUDE · CHECK ENGINE · +1");
  assert.equal(claude["aria-label"], "CLAUDE · CHECK ENGINE · SENSOR OFFLINE");
  assert.equal(claude.dataset.providerDegraded, "true");
  assert.equal(codex.label.textContent, "");

  setProviderIssue("claude", "engine", "CHECK ENGINE", false, documentRoot);
  assert.equal(claude.label.textContent, "CLAUDE · SENSOR OFFLINE");
});

test("Codex availability replaces the bootstrap unavailable label", () => {
  const codex = providerRoot("codex", false);
  const documentRoot = { querySelector: () => codex };

  assert.equal(setProviderAvailability("codex", true, documentRoot), true);
  assert.equal(codex.dataset.providerAvailable, "true");
  assert.equal(codex.label.textContent, "CODEX");

  setProviderAvailability("codex", false, documentRoot);
  assert.equal(codex.label.textContent, "CODEX · UNAVAILABLE");
});
