// Page 3 — MFD settings. Front-end only (localStorage, mfd-settings.js):
// which page opens by default, and which optional pages are in the cycle.
// Deliberately no project filter / cost-mode toggle here yet (D-review:
// those would need a mutable Rust poll-settings state shared with the
// continuous blocks poll — not justified for a first pass).

import { hintOnHover } from "./header-hint.js";
import { loadMfdSettings, saveMfdSettings } from "./mfd-settings.js";

const PAGE_OPTIONS = [
  { value: 0, label: "SINCE START" },
  { value: 1, label: "HISTORY" },
  { value: 2, label: "LIMITS" },
];

/** Custom dropdown, not a native <select> (D-review): WKWebView renders a
 *  <select>'s popup as native OS chrome outside CSS's reach — a stray blue
 *  focus ring in an otherwise all-amber UI. Built from a button + a plain
 *  list instead, so it stays inside the VFD skin end to end. */
function wireDefaultPageDropdown() {
  const root = document.getElementById("default-page-dropdown");
  const btn = document.getElementById("default-page-btn");
  const valueEl = document.getElementById("default-page-value");
  const list = document.getElementById("default-page-list");
  hintOnHover(btn, "Screen shown when the panel opens");

  function paint(value) {
    const opt = PAGE_OPTIONS.find((o) => o.value === value) ?? PAGE_OPTIONS[0];
    valueEl.textContent = opt.label;
    list.querySelectorAll("li").forEach((li) => {
      li.classList.toggle("active", Number(li.dataset.value) === opt.value);
    });
  }

  function close() {
    list.hidden = true;
    root.classList.remove("open");
  }

  btn.onclick = (e) => {
    e.stopPropagation();
    const wasOpen = !list.hidden;
    close();
    if (!wasOpen) {
      list.hidden = false;
      root.classList.add("open");
    }
  };

  list.querySelectorAll("li").forEach((li) => {
    li.onclick = () => {
      const value = Number(li.dataset.value);
      paint(value);
      saveMfdSettings({ defaultPage: value });
      close();
    };
  });

  // Click anywhere outside closes it — same pattern a native <select> gives for free.
  document.addEventListener("click", (e) => {
    if (!root.contains(e.target)) close();
  });

  paint(loadMfdSettings().defaultPage);
}

export function wireSettingsPage() {
  wireDefaultPageDropdown();

  const historyChk = document.getElementById("toggle-history");
  const limitsChk = document.getElementById("toggle-limits");

  const s = loadMfdSettings();
  historyChk.checked = s.showHistory;
  limitsChk.checked = s.showLimits;

  // Hint on the whole label (not just the input) so the text is hoverable too.
  hintOnHover(historyChk.closest("label"), "Show History in the screen cycle");
  hintOnHover(limitsChk.closest("label"), "Show Limits in the screen cycle");

  historyChk.onchange = () => saveMfdSettings({ showHistory: historyChk.checked });
  limitsChk.onchange = () => saveMfdSettings({ showLimits: limitsChk.checked });
}
