import test from "node:test";
import assert from "node:assert/strict";

import { createLatestRequestGate, diagnosticLines } from "./provider-diagnostics.js";

test("diagnostics expose compatibility, runtime and capability provenance", () => {
  const lines = diagnosticLines([{
    provider: "codex",
    compatibility: "partial",
    surface: "Codex CLI",
    runtimeExecutable: "/usr/local/bin/codex",
    runtimeVersion: "codex-cli 0.144.6",
    relatedRuntimes: [{
      surface: "ChatGPT desktop",
      productVersion: "26.715.31925",
      runtimeExecutable: "/Applications/ChatGPT.app/Contents/Resources/codex",
      runtimeVersion: "codex-cli 0.145.0-alpha.18",
    }],
    capabilities: [{
      id: "account-usage",
      status: "unavailable",
      source: "Codex App Server",
      quality: "official",
      fallback: "ccusage codex",
      reason: "authentication unsupported",
      remediation: "Use ChatGPT authentication.",
    }],
  }]);
  assert.equal(lines[0].label, "CODEX · PARTIAL");
  assert.match(lines[1].value, /0\.144\.6/);
  assert.equal(lines[2].label, "CHATGPT DESKTOP");
  assert.match(lines[2].value, /product 26\.715\.31925/);
  assert.match(lines[2].value, /0\.145\.0-alpha\.18/);
  assert.match(lines[3].value, /UNAVAILABLE · OFFICIAL/);
  assert.match(lines[4].value, /fallback: ccusage codex/);
  assert.match(lines[4].value, /Use ChatGPT authentication/);
});

test("available capabilities do not invent remediation rows", () => {
  const lines = diagnosticLines([{
    provider: "claude",
    compatibility: "compatible",
    surface: "Claude Code",
    capabilities: [{ id: "limits", status: "available", source: "statusLine", quality: "official" }],
  }]);
  assert.equal(lines.length, 3);
  assert.equal(lines[2].value, "AVAILABLE · OFFICIAL · statusLine");
});

test("only the latest diagnostics request may render", () => {
  const gate = createLatestRequestGate();
  const first = gate.begin();
  const second = gate.begin();
  assert.equal(gate.isCurrent(first), false);
  assert.equal(gate.isCurrent(second), true);
  gate.cancel();
  assert.equal(gate.isCurrent(second), false);
});
