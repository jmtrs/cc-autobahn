// Configurable keyboard shortcuts for the permission gate's 3 actions
// (Approve/Deny/Always Allow). Front-end only (localStorage, app-settings.js)
// — the listener only runs while the panel itself has keyboard focus, no
// tauri-plugin-global-shortcut, no OS Accessibility permission needed (D16:
// zero new deps).

import { loadGlobalSetting, saveGlobalSetting } from "./app-settings.js";

export const DEFAULTS = Object.freeze({ enabled: true, approve: "A", deny: "D", approveAlways: "W" });

export const ACTIONS = Object.freeze([
  { id: "approve", label: "Approve" },
  { id: "deny", label: "Deny" },
  { id: "approveAlways", label: "Always Allow" },
]);

const MODIFIER_KEYS = new Set(["Shift", "Control", "Alt", "Meta"]);

// Named keys keep their own label instead of being upper-cased (`enter` -> "ENTER"
// reads fine, but arrow keys need the symbol form to stay compact).
const NAMED_KEYS = { ArrowUp: "↑", ArrowDown: "↓", ArrowLeft: "←", ArrowRight: "→", " ": "Space" };

/** Canonical wire format for a chord: "Ctrl+Alt+Shift+Meta+KEY", modifiers
 *  only included when held, fixed order so two equivalent chords always
 *  serialize identically regardless of which order the browser reports them. */
export function normalizeCombo(event) {
  if (MODIFIER_KEYS.has(event.key)) return null;
  const parts = [];
  if (event.ctrlKey) parts.push("Ctrl");
  if (event.altKey) parts.push("Alt");
  if (event.shiftKey) parts.push("Shift");
  if (event.metaKey) parts.push("Meta");
  const key = NAMED_KEYS[event.key] ?? (event.key.length === 1 ? event.key.toUpperCase() : event.key);
  parts.push(key);
  return parts.join("+");
}

const DISPLAY_SYMBOLS = { Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Meta: "⌘" };

/** Renders a stored combo string with macOS modifier glyphs, e.g. "Shift+A" -> "⇧A". */
export function comboLabel(combo) {
  if (!combo) return "—";
  return combo
    .split("+")
    .map((part) => DISPLAY_SYMBOLS[part] ?? part)
    .join("");
}

function validCombo(value) {
  return typeof value === "string" && value.length > 0;
}

export function loadKeybindingSettings() {
  const stored = loadGlobalSetting("keybindings") ?? {};
  const result = { ...DEFAULTS };
  if (typeof stored.enabled === "boolean") result.enabled = stored.enabled;
  for (const action of ACTIONS) {
    if (validCombo(stored[action.id])) result[action.id] = stored[action.id];
  }
  return result;
}

export function saveKeybindingSettings(patch) {
  const next = { ...loadKeybindingSettings(), ...patch };
  return saveGlobalSetting("keybindings", next);
}

/** Matches a keydown event against the current bindings; returns the action
 *  id or null. Never matches on a bare modifier keypress. */
export function matchAction(event, bindings) {
  const combo = normalizeCombo(event);
  if (!combo) return null;
  const entry = ACTIONS.find((action) => bindings[action.id] === combo);
  return entry?.id ?? null;
}
