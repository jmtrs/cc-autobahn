const STORAGE_KEY = "cc-autobahn.settings";
const SCHEMA_VERSION = 2;

const LEGACY_KEYS = Object.freeze({
  mfd: "cc-autobahn.mfd-settings",
  theme: "cc-autobahn.theme",
  permissionSound: "cc-autobahn.permission-sound",
  footerMetric: "cc-autobahn.footerMetric",
  nameplates: "cc-autobahn.nameplates",
});

const DEFAULTS = Object.freeze({
  schemaVersion: SCHEMA_VERSION,
  displayMode: "claude",
  global: {
    mfd: { defaultPage: 0, showHistory: true, showLimits: true, screenOrder: [1, 2] },
    theme: { themeId: "amber", customAccent: "#ff9a1f" },
    permissionSound: { soundId: "chime", customDataUrl: null },
    footerMetric: "pace",
    nameplates: {},
  },
  providers: { claude: {}, codex: {} },
});

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function readJson(key) {
  try {
    return JSON.parse(localStorage.getItem(key));
  } catch {
    return null;
  }
}

function object(value) {
  return value && typeof value === "object" && !Array.isArray(value) ? value : {};
}

function normalize(candidate) {
  const source = object(candidate);
  const global = object(source.global);
  const providers = object(source.providers);
  const displayMode = ["claude", "codex", "both"].includes(source.displayMode)
    ? source.displayMode
    : DEFAULTS.displayMode;
  return {
    schemaVersion: SCHEMA_VERSION,
    displayMode,
    global: {
      mfd: { ...DEFAULTS.global.mfd, ...object(global.mfd) },
      theme: { ...DEFAULTS.global.theme, ...object(global.theme) },
      permissionSound: {
        ...DEFAULTS.global.permissionSound,
        ...object(global.permissionSound),
      },
      footerMetric: global.footerMetric === "autonomy" ? "autonomy" : "pace",
      nameplates: { ...object(global.nameplates) },
    },
    providers: {
      claude: { ...object(providers.claude) },
      codex: { ...object(providers.codex) },
    },
  };
}

function migrateLegacy() {
  const nameplates = object(readJson(LEGACY_KEYS.nameplates));
  const qualifiedNameplates = Object.fromEntries(
    Object.entries(nameplates).map(([model, label]) => [`claude:${model}`, label]),
  );
  return normalize({
    global: {
      mfd: object(readJson(LEGACY_KEYS.mfd)),
      theme: object(readJson(LEGACY_KEYS.theme)),
      permissionSound: object(readJson(LEGACY_KEYS.permissionSound)),
      footerMetric: localStorage.getItem(LEGACY_KEYS.footerMetric),
      nameplates: qualifiedNameplates,
    },
  });
}

function persist(settings) {
  localStorage.setItem(STORAGE_KEY, JSON.stringify(settings));
  return settings;
}

export function loadAppSettings() {
  const stored = readJson(STORAGE_KEY);
  if (stored?.schemaVersion === SCHEMA_VERSION) return normalize(stored);
  return persist(migrateLegacy());
}

export function saveAppSettings(patch) {
  const current = loadAppSettings();
  return persist(normalize({ ...current, ...patch }));
}

export function loadGlobalSetting(key) {
  const value = loadAppSettings().global[key];
  return value && typeof value === "object" ? clone(value) : value;
}

export function saveGlobalSetting(key, value) {
  const current = loadAppSettings();
  current.global[key] = value;
  return persist(normalize(current)).global[key];
}

export function loadDisplayMode() {
  return loadAppSettings().displayMode;
}

export function saveDisplayMode(displayMode) {
  return saveAppSettings({ displayMode }).displayMode;
}

export const SETTINGS_SCHEMA_VERSION = SCHEMA_VERSION;
export const SETTINGS_STORAGE_KEY = STORAGE_KEY;
