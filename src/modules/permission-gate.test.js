import assert from "node:assert/strict";
import test from "node:test";

function fakeElement() {
  const classes = new Set();
  return {
    hidden: false,
    textContent: "",
    offsetWidth: 1,
    onclick: null,
    classList: {
      add: (name) => classes.add(name),
      remove: (name) => classes.delete(name),
      contains: (name) => classes.has(name),
    },
    addEventListener() {},
    contains: () => false,
    querySelector: () => fakeElement(),
  };
}

const ids = [
  "permission-gate",
  "permission-provider",
  "permission-kind",
  "permission-tool",
  "permission-badge",
  "permission-context",
  "permission-summary",
  "permission-cwd",
  "permission-timeout",
  "permission-desktop-status",
  "permission-deny",
  "permission-split",
  "permission-chevron",
  "permission-always-list",
];
const elements = Object.fromEntries(ids.map((id) => [id, fakeElement()]));
elements["permission-gate"].hidden = true;

globalThis.document = {
  getElementById: (id) => elements[id],
};
globalThis.localStorage = {
  getItem: () => null,
  setItem() {},
};
globalThis.Audio = class {
  play() {
    return Promise.resolve();
  }
};

const {
  onDesktopPermissionPending,
  onDesktopPermissionResolved,
  onPermissionPending,
  onPermissionResolved,
} = await import("./permission-gate.js");

test("desktop permission is informational and disappears on matching output", () => {
  onDesktopPermissionPending({
    id: "desktop-call-1",
    provider: "codex",
    toolName: "Command",
    toolInputSummary: "touch /tmp/test",
    cwd: "/tmp/project",
  });

  assert.equal(elements["permission-gate"].hidden, false);
  assert.equal(elements["permission-provider"].textContent, "CODEX");
  assert.equal(elements["permission-kind"].textContent, "DESKTOP PERMISSION");
  assert.equal(elements["permission-desktop-status"].hidden, false);
  assert.equal(elements["permission-desktop-status"].textContent, "Resolve in ChatGPT Desktop");
  assert.equal(elements["permission-deny"].hidden, true);
  assert.equal(elements["permission-split"].hidden, true);
  assert.equal(elements["permission-summary"].textContent, "touch /tmp/test");
  assert.equal(elements["permission-context"].textContent, "project");

  onDesktopPermissionResolved({ id: "another-call" });
  assert.equal(elements["permission-gate"].hidden, false);
  onDesktopPermissionResolved({ id: "desktop-call-1" });
  assert.equal(elements["permission-gate"].hidden, true);
});

test("actionable hook keeps its controls and signals a concurrent Desktop permission", () => {
  onPermissionPending({
    id: "hook-call",
    provider: "claude",
    toolName: "Bash",
    toolInputSummary: "npm test",
    cwd: "/tmp/project",
    project: "project",
    branch: "develop",
    pendingCount: 1,
    providerPendingCount: 1,
    alwaysAllowAvailable: true,
    expiresAtMs: Date.now() + 60_000,
  });
  onDesktopPermissionPending({
    id: "desktop-call-2",
    provider: "codex",
    toolName: "Command",
    toolInputSummary: "touch /tmp/test-2",
    cwd: "/tmp/project",
  });

  assert.equal(elements["permission-kind"].textContent, "PERMISSION REQUEST");
  assert.equal(elements["permission-deny"].hidden, false);
  assert.equal(elements["permission-split"].hidden, false);
  assert.equal(elements["permission-badge"].hidden, false);
  assert.equal(elements["permission-badge"].textContent, "1 DESKTOP");

  onPermissionResolved();
  assert.equal(elements["permission-kind"].textContent, "DESKTOP PERMISSION");
  onDesktopPermissionResolved({ id: "desktop-call-2" });
});
