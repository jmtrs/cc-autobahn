// Shared localStorage-backed settings for the MFD page cycle (Page 3).
// Same pattern already validated for the nameplate override (trip-computer.js):
// front-end only, no backend round-trip needed for a UI preference.

const STORAGE_KEY = "cc-autobahn.mfd-settings";
// screenOrder: display order of the optional pages (1=History, 2=Limits) in
// the cycle — reorderable from Page 3 (D-review). Page 0 (trip computer) and
// 3 (settings) are fixed anchors, first/last, not part of this list.
const DEFAULTS = { defaultPage: 0, showHistory: true, showLimits: true, screenOrder: [1, 2] };

export function loadMfdSettings() {
  try {
    return { ...DEFAULTS, ...JSON.parse(localStorage.getItem(STORAGE_KEY)) };
  } catch {
    return { ...DEFAULTS };
  }
}

export function saveMfdSettings(patch) {
  const next = { ...loadMfdSettings(), ...patch };
  localStorage.setItem(STORAGE_KEY, JSON.stringify(next));
  return next;
}
