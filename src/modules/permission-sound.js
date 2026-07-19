// Settings for the permission-gate alert sound (D42 follow-up). Same
// localStorage load/save shape as mfd-settings.js/theme.js: front-end only,
// no backend round-trip — playing an <audio> element needs no Tauri IPC.

const STORAGE_KEY = "cc-autobahn.permission-sound";
// No separate "enabled" flag (D-review) — OFF is just another dropdown
// entry (soundId: "none"), one less control to keep in sync with the other.
const DEFAULTS = { soundId: "chime", customDataUrl: null };

// Custom uploads are kept as a data: URL directly in localStorage (no Tauri
// fs write) — this cap keeps a single quirky upload from blowing the
// origin's storage quota (~5-10MB, shared with every other setting here).
export const MAX_CUSTOM_SOUND_BYTES = 300_000;

export const BUILTIN_SOUNDS = {
  chime: { label: "CHIME", url: "/sounds/chime.wav" },
  beep: { label: "BEEP", url: "/sounds/beep.wav" },
  click: { label: "CLICK", url: "/sounds/click.wav" },
  seatbelt: { label: "SEATBELT", url: "/sounds/seatbelt-click.wav" },
  handbrake: { label: "HANDBRAKE", url: "/sounds/handbrake.wav" },
  ping: { label: "PING", url: "/sounds/electronic-ping.wav" },
  pop: { label: "POP", url: "/sounds/pop.wav" },
  laser: { label: "LASER", url: "/sounds/laser.wav" },
  crash: { label: "CRASH", url: "/sounds/car-crash.wav" },
};

export function loadPermissionSoundSettings() {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(STORAGE_KEY)) };
  } catch {
    return { ...DEFAULTS };
  }
}

export function savePermissionSoundSettings(patch) {
  const next = { ...loadPermissionSoundSettings(), ...patch };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  return next;
}

/** Resolves the audio src for a given (soundId, customDataUrl) pair without
 *  touching localStorage — shared by the real playback path and Settings'
 *  play-on-select preview of the in-progress choice before it's saved. */
function resolveSrc(soundId, customDataUrl) {
  if (soundId === "custom") return customDataUrl ?? null;
  return BUILTIN_SOUNDS[soundId]?.url ?? null;
}

/** Plays the given (soundId, customDataUrl) pair directly — used by Settings
 *  to preview a choice the instant it's picked from the dropdown, instead of
 *  a separate Test button. `.play()` rejects if the webview hasn't seen a
 *  user gesture yet this session (WKWebView autoplay policy); swallowed on
 *  purpose, same fails-open spirit as the rest of this project (D42) — no
 *  sound is a silent degradation, not an error to surface. */
export function previewPermissionSound(soundId, customDataUrl) {
  const src = resolveSrc(soundId, customDataUrl);
  if (!src) return;
  new Audio(src).play().catch(() => {});
}

/** Plays the current settings' sound, honoring `soundId: "none"` (OFF). */
export function playPermissionSound() {
  const s = loadPermissionSoundSettings();
  if (s.soundId === "none") return;
  previewPermissionSound(s.soundId, s.customDataUrl);
}
