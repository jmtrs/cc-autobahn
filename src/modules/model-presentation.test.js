import assert from "node:assert/strict";
import test from "node:test";

import {
  defaultNameplate,
  formatProviderModelCode,
  modelSlots,
  resolveModelPresentation,
} from "./model-presentation.js";

test("Claude keeps its established family selector and W203 nameplates", () => {
  assert.deepEqual(modelSlots("claude").map(({ key, code }) => [key, code]), [
    ["opus", "O"], ["sonnet", "S"], ["haiku", "H"], ["fable", "F"], ["custom", "C"],
  ]);
  assert.deepEqual(resolveModelPresentation("claude", "claude-haiku-4-5-20251001"), {
    modelKey: "haiku",
    slotKey: "haiku",
    nameplate: "CC 220 CDI",
    code: "H",
    editable: true,
  });
  assert.equal(defaultNameplate("claude", "opus"), "CC 500");
});

test("Codex exposes real GPT family labels and distinguishable compact codes", () => {
  assert.deepEqual(modelSlots("codex").map(({ key, code }) => [key, code]), [
    ["gpt", "G"], ["sol", "S"], ["terra", "T"], ["mini", "M"], ["custom", "C"],
  ]);
  assert.deepEqual(resolveModelPresentation("codex", "gpt-5.6-sol"), {
    modelKey: "gpt-5.6-sol",
    slotKey: "sol",
    nameplate: "GPT 5.6 SOL",
    code: "5.6S",
    editable: true,
  });
  assert.equal(formatProviderModelCode("codex", "gpt-5.6-terra"), "5.6T");
  assert.equal(formatProviderModelCode("codex", "gpt-5.6-mini"), "5.6M");
});

test("unknown provider models keep deterministic honest fallbacks", () => {
  assert.deepEqual(resolveModelPresentation("codex", "acme/model-x"), {
    modelKey: "acme/model-x",
    slotKey: "custom",
    nameplate: "ACME MODEL X",
    code: "ACME",
    editable: false,
  });
  assert.equal(formatProviderModelCode("claude", "glm-5.2"), "GLM5");
  assert.equal(resolveModelPresentation("codex", ""), null);
});
