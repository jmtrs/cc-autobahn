// Permission gate (D42): approve/deny Claude Code's PermissionRequest hook
// straight from the cluster instead of alt-tabbing to a terminal. Same
// sustained-panel + edge-triggered-pulse idiom as redline.js, but its own
// class/color (a distinct, actionable state, not a threshold breach) — and
// NOT engine-overlay.js's full takeover, since a pending permission is
// transient and should coexist with normal use, not block it. The backend
// (permission::mod.rs) already drives the tray badge directly on every queue
// change, so unlike redline.js this module has no tray IPC of its own.

import { playPermissionSound } from "./permission-sound.js";

let gateInvoke = null;
let currentId = null;
let wasVisible = false;
let timeoutTimer = null;

/** Ticks the "auto-clears in Ns" label against the backend's own deadline
 *  for this entry (D-review) — Claude Code has no documented way to tell our
 *  hook a decision was already made elsewhere, so an orphaned request just
 *  sits until the backend's own QUEUE_TIMEOUT_SECS gives up on it. Without
 *  this the panel looked permanently stuck instead of visibly counting down
 *  to its own self-clear. */
function startTimeoutCountdown(expiresAtMs) {
  clearInterval(timeoutTimer);
  const el = document.getElementById("permission-timeout");
  const tick = () => {
    const secs = Math.max(0, Math.round((expiresAtMs - Date.now()) / 1000));
    el.textContent = `auto-clears in ${secs}s`;
    if (secs <= 0) clearInterval(timeoutTimer);
  };
  tick();
  timeoutTimer = setInterval(tick, 1000);
}

/** Re-triggerable one-shot animation, same pattern as redline.js's pulseOnce. */
function pulseOnce(el, className) {
  if (!el) return;
  el.classList.remove(className);
  void el.offsetWidth;
  el.classList.add(className);
  el.addEventListener("animationend", () => el.classList.remove(className), { once: true });
}

/** Wires the Approve/Deny buttons + the Always-Allow split menu. Guarded:
 *  no-op outside Tauri. */
export async function wirePermissionGate() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { invoke } = await import("@tauri-apps/api/core");
  gateInvoke = invoke;

  document.getElementById("permission-approve").onclick = () => resolve("permission_approve");
  document.getElementById("permission-deny").onclick = () => resolve("permission_deny");

  // Hydrates from the current queue state instead of only ever reacting to
  // `permission-pending` — a request queued before this module's `listen()`
  // subscription (ipc-events.js) attaches, or a webview reload while one is
  // already pending, would otherwise leave the gate panel unreachable even
  // though the tray badge is correctly blinking (D42 review fix).
  try {
    const snapshot = await gateInvoke("permission_pending_snapshot");
    if (snapshot) onPermissionPending(snapshot);
  } catch (e) {
    console.error("[permission] pending_snapshot:", e);
  }

  // Split-button menu — same open/close idiom as the Page-3 dropdowns
  // (settings-page.js): stopPropagation + click-anywhere-outside closes.
  const root = document.getElementById("permission-split");
  const chevron = document.getElementById("permission-chevron");
  const list = document.getElementById("permission-always-list");

  chevron.onclick = (e) => {
    e.stopPropagation();
    const wasOpen = !list.hidden;
    closeAlwaysMenu();
    if (!wasOpen) {
      list.hidden = false;
      root.classList.add("open");
    }
  };
  document.getElementById("permission-approve-always").onclick = () => {
    closeAlwaysMenu();
    resolve("permission_approve_always");
  };
  document.addEventListener("click", (e) => {
    if (!root.contains(e.target)) closeAlwaysMenu();
  });
}

function closeAlwaysMenu() {
  document.getElementById("permission-always-list").hidden = true;
  document.getElementById("permission-split").classList.remove("open");
}

async function resolve(command) {
  if (!gateInvoke || !currentId) return;
  const id = currentId;
  // Null out BEFORE the await: a fast double-click would otherwise fire a
  // second invoke with the same id, and the backend rightly errors with
  // "no such pending request" on it — noise in the console, not a real
  // failure (the first click already resolved it).
  currentId = null;
  try {
    await gateInvoke(command, { id });
  } catch (e) {
    // The invoke can fail before the backend sees it (IPC/runtime error). Keep
    // the current card actionable instead of leaving visible buttons wired to
    // a null id forever. A newer pending event wins and must not be overwritten.
    if (currentId === null) {
      currentId = id;
      clearInterval(timeoutTimer);
      document.getElementById("permission-timeout").textContent = "action failed — retry";
    }
    console.error(`[permission] ${command}:`, e);
  }
}

/** `permission-pending` event: always the current head of the queue + a
 *  count, whether this arrival became the visible request or just landed
 *  behind an existing one — repaint unconditionally from the payload. */
export function onPermissionPending(payload) {
  currentId = payload.id;
  document.getElementById("permission-tool").textContent = payload.toolName;
  // Shell-prompt cue for Bash only — other tools show the raw field
  // (a file path reads fine on its own, "$ " would be misleading).
  const summary =
    payload.toolName === "Bash" ? `$ ${payload.toolInputSummary}` : payload.toolInputSummary;
  document.getElementById("permission-summary").textContent = summary;
  document.getElementById("permission-cwd").textContent = payload.cwd;

  // Context row (top-right): "project · branch" — same at-a-glance session
  // identity AgentNotch rows give. Branch is absent outside a git repo.
  document.getElementById("permission-context").textContent = payload.branch
    ? `${payload.project} · ${payload.branch}`
    : payload.project;

  const badge = document.getElementById("permission-badge");
  const more = payload.pendingCount - 1;
  badge.hidden = more <= 0;
  if (more > 0) badge.textContent = `+${more} more`;

  // The chevron only appears when the backend found a rule-able field for
  // this tool (Bash command / file path) — without one there's no safe
  // "Always Allow" rule to build. Also close a menu left open by a previous
  // request: this payload is a fresh card, its state must not leak over.
  closeAlwaysMenu();
  document.getElementById("permission-chevron").hidden = !payload.alwaysAllowAvailable;

  startTimeoutCountdown(payload.expiresAtMs);

  const gate = document.getElementById("permission-gate");
  gate.hidden = false;
  // Pulsed on the card, not the overlay itself — the static glow (style.css)
  // lives on .sensor-card too, so the flash modifies the same box-shadow
  // instead of stacking a second one on an element that has none by default.
  // Sound gated the same way: only the hidden→visible transition, not every
  // request that stacks behind it (those are covered by the "+n more" badge).
  if (!wasVisible) {
    pulseOnce(gate.querySelector(".sensor-card"), "permission-gate-enter");
    playPermissionSound();
  }
  wasVisible = true;
}

/** `permission-resolved` event: queue is empty, hide the panel. */
export function onPermissionResolved() {
  currentId = null;
  wasVisible = false;
  clearInterval(timeoutTimer);
  closeAlwaysMenu();
  document.getElementById("permission-gate").hidden = true;
}
