// Color themes (Page 3 — Settings). Front-end only (localStorage), same
// load/save-patch pattern as mfd-settings.js. Presets are hand-authored hex
// values; the "custom" theme derives everything from one accent color via
// HSL so a single native <input type="color"> is enough (D-review: 5 manual
// swatches don't fit cleanly in the 440x150 window).

import { loadGlobalSetting, saveGlobalSetting } from "./app-settings.js";
const DEFAULTS = { themeId: "amber", customAccent: "#ff9a1f" };

export const PRESETS = {
  amber: {
    label: "AMBER",
    amber: "#ff9a1f",
    amberDim: "#7a3d08",
    amberGlow: "#ffb347",
    bg: "#0a0705",
    bezel: "#17120d",
  },
  emerald: {
    label: "EMERALD",
    amber: "#2dd881",
    amberDim: "#0d4d29",
    amberGlow: "#7dffb0",
    bg: "#050a07",
    bezel: "#0d1712",
  },
  ice: {
    label: "ICE",
    amber: "#4fc3ff",
    amberDim: "#123a52",
    amberGlow: "#9fe0ff",
    bg: "#050a0f",
    bezel: "#0d1620",
  },
  ruby: {
    label: "RUBY",
    amber: "#ff3b30",
    amberDim: "#5c0e0b",
    amberGlow: "#ff7a70",
    bg: "#0a0505",
    bezel: "#170d0d",
  },
};

export function loadThemeSettings() {
  return { ...DEFAULTS, ...loadGlobalSetting("theme") };
}

export function saveThemeSettings(patch) {
  const next = { ...loadThemeSettings(), ...patch };
  return saveGlobalSetting("theme", next);
}

function hexToHsl(hex) {
  const r = parseInt(hex.slice(1, 3), 16) / 255;
  const g = parseInt(hex.slice(3, 5), 16) / 255;
  const b = parseInt(hex.slice(5, 7), 16) / 255;
  const max = Math.max(r, g, b);
  const min = Math.min(r, g, b);
  const l = (max + min) / 2;
  if (max === min) return { h: 0, s: 0, l: l * 100 };
  const d = max - min;
  const s = l > 0.5 ? d / (2 - max - min) : d / (max + min);
  let h;
  switch (max) {
    case r: h = (g - b) / d + (g < b ? 6 : 0); break;
    case g: h = (b - r) / d + 2; break;
    default: h = (r - g) / d + 4;
  }
  return { h: h * 60, s: s * 100, l: l * 100 };
}

function hslToHex(h, s, l) {
  s /= 100;
  l /= 100;
  const k = (n) => (n + h / 30) % 12;
  const a = s * Math.min(l, 1 - l);
  const f = (n) => l - a * Math.max(-1, Math.min(k(n) - 3, Math.min(9 - k(n), 1)));
  const toHex = (n) => Math.round(f(n) * 255).toString(16).padStart(2, "0");
  return `#${toHex(0)}${toHex(8)}${toHex(4)}`;
}

const clamp = (v, min, max) => Math.max(min, Math.min(max, v));

/** Derives the 5 palette colors from a single accent color. Mirrors the
 *  relationship between AMBER's own hand-authored values (dim ≈ half
 *  lightness, glow ≈ +12pt lightness, bg/bezel near-black with a hue tint).
 *  bg/bezel are pinned near-black regardless of accent, so a very dark accent
 *  pick (l below the floor) would otherwise sit at nearly the same lightness
 *  as the background — floor keeps --amber legible against --bg and keeps
 *  --amber-dim (floor 15-30) from ending up brighter than --amber itself. */
function deriveFromAccent(accentHex) {
  const { h, s, l: rawL } = hexToHsl(accentHex);
  const l = clamp(rawL, 32, 100);
  const amber = hslToHex(h, s, l);
  return {
    amber,
    amberDim: hslToHex(h, s, clamp(l * 0.45, 15, 30)),
    amberGlow: hslToHex(h, Math.min(s, 100), clamp(l + 15, 0, 88)),
    bg: hslToHex(h, Math.min(s, 40), 3),
    bezel: hslToHex(h, Math.min(s, 40), 7),
  };
}

export function resolveColors(settings) {
  if (settings.themeId === "custom") return deriveFromAccent(settings.customAccent);
  return PRESETS[settings.themeId] ?? PRESETS.amber;
}

function hexToRgbTriplet(hex) {
  const r = parseInt(hex.slice(1, 3), 16);
  const g = parseInt(hex.slice(3, 5), 16);
  const b = parseInt(hex.slice(5, 7), 16);
  return `${r}, ${g}, ${b}`;
}

/** App cursors: a VFD triangle + text-beam (same glyph language as ▸/▲ in
 *  the UI) baked into SVG data-URIs, since CSS cursor images/background-
 *  images can't read CSS vars. Used as the background-image of the
 *  synthetic .fake-cursor element (cursor.js) — not the native `cursor:`
 *  property. That's deliberate everywhere, not just for the z-index trick
 *  (sitting below the screen's scanline grid): WebKit's UA stylesheet gives
 *  native form controls their own cursor that plain CSS can't always beat,
 *  and the same turned out true for the nameplate's `cursor: text` while
 *  renaming — so every visual state, including the text-beam, is rendered
 *  by us rather than relying on any native cursor showing up at all.
 *  Both shapes share one two-layer draw: a wide `--bg`-colored halo behind
 *  (rounded joins, so it never spikes past the canvas) then the crisp
 *  amber shape on top (sharp miter joins for the arrow — a true triangle,
 *  no rounding). Without the halo the cursor could blend into same-hue UI
 *  (an active dropdown row, a themed accent) since it shares the theme's
 *  amber; the dark backdrop guarantees contrast against any surface color.
 *  22x22 canvas for both. Re-baked on every applyColors so they re-tint
 *  with the theme. */
function svgCursorUrl(inner) {
  return `url("data:image/svg+xml;utf8,<svg xmlns='http://www.w3.org/2000/svg' width='22' height='22' viewBox='0 0 22 22'>${inner}</svg>")`;
}

/** Arrow: tilted -25° (leans like the system arrow). Hotspot at the
 *  rotated apex (8,6). */
function arrowCursor(fill, stroke, bg) {
  const f = fill === "none" ? "none" : fill.replace("#", "%23");
  const s = stroke.replace("#", "%23");
  const b = bg.replace("#", "%23");
  const d = "M11 5 L17 17 L5 17 Z";
  const rotate = "rotate(-25 11 11)";
  return svgCursorUrl(
    `<path d='${d}' transform='${rotate}' fill='none' stroke='${b}' stroke-width='3.5' stroke-linejoin='round'/>` +
      `<path d='${d}' transform='${rotate}' fill='${f}' stroke='${s}' stroke-width='1.5' stroke-linejoin='miter'/>`,
  );
}

/** Text-beam for in-place editing (nameplate rename): a plain thin vertical
 *  stick — a serif I-beam (tried first) and even open strokes with serifs
 *  both read as too busy for something this small; a minimal caret line is
 *  the actual VFD-simple answer. Centered on the (8,6) hotspot so it lines
 *  up with the arrow state it swaps with. */
function beamCursor(color, bg) {
  const c = color.replace("#", "%23");
  const b = bg.replace("#", "%23");
  const d = "M8 1 V11";
  return svgCursorUrl(
    `<path d='${d}' fill='none' stroke='${b}' stroke-width='3' stroke-linecap='round'/>` +
      `<path d='${d}' fill='none' stroke='${c}' stroke-width='1.2' stroke-linecap='round'/>`,
  );
}

export function applyColors(colors) {
  const root = document.documentElement.style;
  root.setProperty("--amber", colors.amber);
  root.setProperty("--amber-dim", colors.amberDim);
  root.setProperty("--amber-glow", colors.amberGlow);
  root.setProperty("--bg", colors.bg);
  root.setProperty("--bezel", colors.bezel);
  root.setProperty("--amber-rgb", hexToRgbTriplet(colors.amber));
  root.setProperty("--amber-glow-rgb", hexToRgbTriplet(colors.amberGlow));
  root.setProperty("--bg-rgb", hexToRgbTriplet(colors.bg));
  root.setProperty("--cursor-base", arrowCursor("none", colors.amberDim, colors.bg));
  root.setProperty("--cursor-click", arrowCursor(colors.amber, colors.amber, colors.bg));
  root.setProperty("--cursor-text", beamCursor(colors.amber, colors.bg));
}

export function applyTheme(themeId) {
  const settings = saveThemeSettings({ themeId });
  applyColors(resolveColors(settings));
}

export function initTheme() {
  applyColors(resolveColors(loadThemeSettings()));
}
