import assert from "node:assert/strict";
import test from "node:test";

import { changeDisplayMode, initializeDisplayMode, paintDisplayMode } from "./display-mode.js";
import { state } from "./telemetry-state.js";

function fixture() {
  const nameplate = { textContent: "", contentEditable: "false" };
  const tag = { textContent: "" };
  const chassis = {
    dataset: {},
    querySelector(selector) {
      if (selector.includes("active-provider-tag")) return tag;
      if (selector.includes("nameplate")) return nameplate;
      return null;
    },
  };
  const roots = [
    { hidden: false, dataset: { providerModule: "claude" } },
    { hidden: false, dataset: { providerModule: "codex" } },
  ];
  const events = [];
  return {
    chassis,
    roots,
    nameplate,
    tag,
    documentRoot: {
      querySelector: () => chassis,
      querySelectorAll: () => roots,
      dispatchEvent: (event) => events.push(event),
    },
  };
}

test("three display modes preserve mounted roots and only change visibility", () => {
  const ui = fixture();

  paintDisplayMode("claude", ui.documentRoot);
  assert.deepEqual(ui.roots.map((root) => root.hidden), [false, true]);

  paintDisplayMode("codex", ui.documentRoot);
  assert.deepEqual(ui.roots.map((root) => root.hidden), [true, false]);
  assert.equal(ui.tag.textContent, "CODEX");
  assert.equal(ui.nameplate.textContent, "—");

  paintDisplayMode("both", ui.documentRoot);
  assert.deepEqual(ui.roots.map((root) => root.hidden), [false, false]);
  assert.equal(state.global.displayMode, "both");
});

test("mode change resizes before applying and persisting", async () => {
  const ui = fixture();
  paintDisplayMode("claude", ui.documentRoot);
  const calls = [];

  await changeDisplayMode("both", {
    documentRoot: ui.documentRoot,
    invoke: async (...args) => calls.push(["invoke", ...args]),
    persist: (mode) => calls.push(["persist", mode]),
  });

  assert.deepEqual(calls, [
    ["invoke", "set_display_mode", { mode: "both" }],
    ["persist", "both"],
  ]);
  assert.equal(ui.chassis.dataset.displayMode, "both");
});

test("persistence failure rolls native size back to the previous mode", async () => {
  const ui = fixture();
  paintDisplayMode("claude", ui.documentRoot);
  const calls = [];

  await assert.rejects(
    changeDisplayMode("both", {
      documentRoot: ui.documentRoot,
      invoke: async (...args) => calls.push(args),
      persist: () => {
        throw new Error("storage failed");
      },
    }),
    /storage failed/,
  );

  assert.deepEqual(calls, [
    ["set_display_mode", { mode: "both" }],
    ["set_display_mode", { mode: "claude" }],
  ]);
  assert.equal(ui.chassis.dataset.displayMode, "claude");
});

test("failed native resize leaves UI and persistence unchanged", async () => {
  const ui = fixture();
  paintDisplayMode("claude", ui.documentRoot);
  let persisted = false;

  await assert.rejects(
    changeDisplayMode("both", {
      documentRoot: ui.documentRoot,
      invoke: async () => {
        throw new Error("resize failed");
      },
      persist: () => {
        persisted = true;
      },
    }),
    /resize failed/,
  );

  assert.equal(ui.chassis.dataset.displayMode, "claude");
  assert.equal(persisted, false);
});

test("startup applies saved mode after native transition", async () => {
  const ui = fixture();
  const calls = [];
  await initializeDisplayMode({
    documentRoot: ui.documentRoot,
    load: () => "codex",
    invoke: async (...args) => calls.push(args),
  });
  assert.deepEqual(calls, [["set_display_mode", { mode: "codex" }]]);
  assert.equal(ui.chassis.dataset.displayMode, "codex");
});

test("startup resize failure falls back to the safe Claude presentation", async () => {
  const ui = fixture();
  await assert.rejects(
    initializeDisplayMode({
      documentRoot: ui.documentRoot,
      load: () => "both",
      invoke: async () => {
        throw new Error("startup resize failed");
      },
    }),
    /startup resize failed/,
  );
  assert.equal(ui.chassis.dataset.displayMode, "claude");
  assert.deepEqual(ui.roots.map((root) => root.hidden), [false, true]);
  assert.equal(state.global.displayMode, "claude");
});
