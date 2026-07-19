import assert from "node:assert/strict";
import test from "node:test";

import { clearProviderRedline, updateRedline } from "./redline.js";

function fakeElement() {
  const classes = new Set();
  return {
    offsetWidth: 1,
    classList: {
      add: (name) => classes.add(name),
      remove: (name) => classes.delete(name),
      toggle(name, active) {
        if (active) classes.add(name);
        else classes.delete(name);
      },
      contains: (name) => classes.has(name),
    },
    addEventListener() {},
    querySelectorAll: () => [],
  };
}

function fakeView(provider) {
  const screen = fakeElement();
  const elements = {
    segments: fakeElement(),
    "footer-metric-value": fakeElement(),
    burn: fakeElement(),
  };
  return {
    provider,
    root: () => screen,
    element: (id) => elements[id],
  };
}

test("redline classes stay local to the provider view", () => {
  const claude = fakeView("claude");
  const codex = fakeView("codex");

  updateRedline(60, null, claude);

  assert.equal(claude.root().classList.contains("redline"), true);
  assert.equal(claude.element("segments").classList.contains("redline"), true);
  assert.equal(codex.root().classList.contains("redline"), false);
  assert.equal(codex.element("segments").classList.contains("redline"), false);

  updateRedline(null, null, codex);
  assert.equal(claude.root().classList.contains("redline"), true);
  assert.equal(codex.root().classList.contains("redline"), false);

  clearProviderRedline(claude);
  updateRedline(null, null, codex);
  assert.equal(codex.root().classList.contains("redline"), false);
});
