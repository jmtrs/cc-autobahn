import test from "node:test";
import assert from "node:assert/strict";

import {
  loadAppSettings,
  loadGlobalSetting,
  saveDisplayMode,
  saveGlobalSetting,
  SETTINGS_SCHEMA_VERSION,
  SETTINGS_STORAGE_KEY,
} from "./app-settings.js";

function storageFixture(entries = {}) {
  const values = new Map(Object.entries(entries));
  globalThis.localStorage = {
    getItem: (key) => values.get(key) ?? null,
    setItem: (key, value) => values.set(key, String(value)),
    removeItem: (key) => values.delete(key),
    clear: () => values.clear(),
  };
  return values;
}

test("legacy settings migrate once into schema v2 without deleting rollback keys", () => {
  const legacyNameplates = JSON.stringify({ opus: "C 500 CUSTOM" });
  const values = storageFixture({
    "cc-autobahn.mfd-settings": JSON.stringify({ defaultPage: 2, screenOrder: [2, 1] }),
    "cc-autobahn.theme": JSON.stringify({ themeId: "emerald" }),
    "cc-autobahn.permission-sound": JSON.stringify({ soundId: "none" }),
    "cc-autobahn.footerMetric": "autonomy",
    "cc-autobahn.nameplates": legacyNameplates,
  });

  const settings = loadAppSettings();
  assert.equal(settings.schemaVersion, SETTINGS_SCHEMA_VERSION);
  assert.equal(settings.displayMode, "claude");
  assert.equal(settings.global.mfd.defaultPage, 2);
  assert.deepEqual(settings.global.mfd.screenOrder, [2, 1]);
  assert.equal(settings.global.theme.themeId, "emerald");
  assert.equal(settings.global.permissionSound.soundId, "none");
  assert.equal(settings.global.footerMetric, "autonomy");
  assert.equal(settings.global.nameplates["claude:opus"], "C 500 CUSTOM");
  assert.deepEqual(settings.providers, { claude: {}, codex: {} });
  assert.equal(values.get("cc-autobahn.nameplates"), legacyNameplates);
  assert.ok(values.has(SETTINGS_STORAGE_KEY));
});

test("v2 settings validate display mode and isolate returned nested values", () => {
  storageFixture({
    [SETTINGS_STORAGE_KEY]: JSON.stringify({
      schemaVersion: 2,
      displayMode: "future-mode",
      global: { theme: { themeId: "ruby" }, nameplates: { "codex:gpt-5": "GT 5" } },
      providers: { codex: { enabled: true } },
    }),
  });

  assert.equal(loadAppSettings().displayMode, "claude");
  assert.equal(saveDisplayMode("both"), "both");
  assert.equal(saveDisplayMode("invalid"), "claude");

  const nameplates = loadGlobalSetting("nameplates");
  nameplates["codex:gpt-5"] = "MUTATED";
  assert.equal(loadGlobalSetting("nameplates")["codex:gpt-5"], "GT 5");

  saveGlobalSetting("theme", { themeId: "ice", customAccent: "#123456" });
  assert.equal(loadGlobalSetting("theme").themeId, "ice");
  assert.deepEqual(loadAppSettings().providers.codex, { enabled: true });
});
