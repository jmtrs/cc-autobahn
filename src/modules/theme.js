// Color themes (Page 3 — Settings). Front-end only (localStorage), same
// load/save-patch pattern as mfd-settings.js. Presets are hand-authored hex
// values; the "custom" theme derives everything from one accent color via
// HSL so a single native <input type="color"> is enough (D-review: 5 manual
// swatches don't fit cleanly in the 440x150 window).

const STORAGE_KEY = "cc-autobahn.theme";
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
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(STORAGE_KEY)) };
  } catch {
    return { ...DEFAULTS };
  }
}

export function saveThemeSettings(patch) {
  const next = { ...loadThemeSettings(), ...patch };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  return next;
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
}

export function applyTheme(themeId) {
  const settings = saveThemeSettings({ themeId });
  applyColors(resolveColors(settings));
}

export function initTheme() {
  applyColors(resolveColors(loadThemeSettings()));
}
