// MFD page cycle (D-review: real trip computers cycle screens with a single
// stalk button instead of cramming everything into one readout). One
// forward-only button, wraps around — same UX as the real W203 stalk.
// Which pages are in the cycle, and which opens by default, come from
// Page 3's settings (mfd-settings.js).

import { hintOnHover } from "./header-hint.js";
import { loadMfdSettings } from "./mfd-settings.js";

const PAGE_LABELS = { 0: "SINCE START", 1: "HISTORY", 2: "LIMITS", 3: "SETTINGS" };

let current = 0;

const SHOW_FLAG = { 1: "showHistory", 2: "showLimits" };

function cycleOrder() {
  const s = loadMfdSettings();
  // Guard against a corrupted/partial screenOrder (e.g. hand-edited
  // localStorage) — falls back to the default pair instead of silently
  // dropping a page from the cycle forever.
  const order = Array.isArray(s.screenOrder) ? s.screenOrder : [1, 2];
  const shown = order.filter((id) => SHOW_FLAG[id] && s[SHOW_FLAG[id]]);
  return [0, ...shown, 3];
}

function activate(page) {
  current = page;
  document.querySelectorAll(".page").forEach((el) => {
    el.classList.toggle("active", Number(el.dataset.page) === page);
  });
  document.getElementById("page-label").textContent = PAGE_LABELS[page];
  document.dispatchEvent(new CustomEvent("mfd-page-changed", { detail: { page } }));
}

export function wireMfdNav() {
  const settings = loadMfdSettings();
  const order = cycleOrder();
  activate(order.includes(settings.defaultPage) ? settings.defaultPage : 0);

  const btn = document.getElementById("mfd-btn");
  hintOnHover(btn, "Cycle to the next screen");
  btn.onclick = () => {
    const order = cycleOrder();
    const idx = order.indexOf(current);
    activate(order[idx === -1 ? 0 : (idx + 1) % order.length]);
  };
}
