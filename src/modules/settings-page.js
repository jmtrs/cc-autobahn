// Page 3 — MFD settings. Front-end only (localStorage, mfd-settings.js):
// which page opens by default, and which optional pages are in the cycle.
// Deliberately no project filter / cost-mode toggle here yet (D-review:
// those would need a mutable Rust poll-settings state shared with the
// continuous blocks poll — not justified for a first pass).

import { hintOnHover } from "./header-hint.js";
import { loadMfdSettings, saveMfdSettings } from "./mfd-settings.js";
import { applyTheme, initTheme, loadThemeSettings, PRESETS, saveThemeSettings } from "./theme.js";

const PAGE_OPTIONS = [
  { value: 0, label: "SINCE START" },
  { value: 1, label: "HISTORY" },
  { value: 2, label: "LIMITS" },
];

// Metadata for the reorderable "SHOW SCREENS" rows (screenOrder, mfd-nav.js).
const SCREEN_META = {
  1: { checkId: "toggle-history", flag: "showHistory", label: "HISTORY", hint: "Show History in the screen cycle" },
  2: { checkId: "toggle-limits", flag: "showLimits", label: "LIMITS", hint: "Show Limits in the screen cycle" },
};

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

/** Swaps `pageId` with its neighbor (delta = -1 up / +1 down) in screenOrder
 *  and re-renders. Real trip computers don't let you drag things around —
 *  up/down buttons on each row fit the same mechanical-button language as
 *  the rest of Page 3 (D10: no drag-and-drop lib, zero new deps). */
function moveScreen(pageId, delta) {
  const order = [...loadMfdSettings().screenOrder];
  const idx = order.indexOf(pageId);
  const target = idx + delta;
  if (idx === -1 || target < 0 || target >= order.length) return;
  [order[idx], order[target]] = [order[target], order[idx]];
  saveMfdSettings({ screenOrder: order });
  renderScreenList();
}

/** Rebuilds the "SHOW SCREENS" rows in screenOrder's order, each with its own
 *  checkbox (still toggles showHistory/showLimits) plus up/down reorder
 *  buttons — top row's up and bottom row's down are disabled. */
function renderScreenList() {
  const s = loadMfdSettings();
  const list = document.getElementById("screen-order-list");
  list.innerHTML = "";
  s.screenOrder.forEach((pageId, i) => {
    const meta = SCREEN_META[pageId];
    if (!meta) return;
    const row = document.createElement("div");
    row.className = "vfd-reorder-row";
    row.innerHTML = `
      <label class="vfd-check"><input type="checkbox" id="${meta.checkId}" ${s[meta.flag] ? "checked" : ""} /> ${meta.label}</label>
      <span class="vfd-reorder-btns">
        <button type="button" class="vfd-reorder-btn" data-dir="up" ${i === 0 ? "disabled" : ""}>&#9650;</button>
        <button type="button" class="vfd-reorder-btn" data-dir="down" ${i === s.screenOrder.length - 1 ? "disabled" : ""}>&#9660;</button>
      </span>`;

    const chk = row.querySelector("input");
    hintOnHover(chk.closest("label"), meta.hint);
    chk.onchange = () => saveMfdSettings({ [meta.flag]: chk.checked });
    row.querySelector('[data-dir="up"]').onclick = () => moveScreen(pageId, -1);
    row.querySelector('[data-dir="down"]').onclick = () => moveScreen(pageId, 1);
    list.appendChild(row);
  });
}

const THEME_OPTIONS = [
  ...Object.entries(PRESETS).map(([value, preset]) => ({ value, label: preset.label })),
  { value: "custom", label: "CUSTOM" },
];

/** Same custom-dropdown pattern as wireDefaultPageDropdown, plus a single
 *  accent-color picker that only shows up for the CUSTOM entry (theme.js
 *  derives the other 4 palette variables from that one color). */
function wireThemeSection() {
  const root = document.getElementById("theme-dropdown");
  const btn = document.getElementById("theme-btn");
  const valueEl = document.getElementById("theme-value");
  const list = document.getElementById("theme-list");
  const accentRow = document.getElementById("theme-accent-row");
  const accentInput = document.getElementById("theme-accent-input");
  hintOnHover(btn, "Instrument cluster color palette");
  hintOnHover(accentRow, "Pick your own accent color");

  list.innerHTML = THEME_OPTIONS.map(
    (opt) => `<li data-value="${opt.value}">${opt.label}</li>`
  ).join("");

  function paint(themeId) {
    const opt = THEME_OPTIONS.find((o) => o.value === themeId) ?? THEME_OPTIONS[0];
    valueEl.textContent = opt.label;
    list.querySelectorAll("li").forEach((li) => {
      li.classList.toggle("active", li.dataset.value === opt.value);
    });
    accentRow.hidden = themeId !== "custom";
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
      const themeId = li.dataset.value;
      paint(themeId);
      applyTheme(themeId);
      close();
    };
  });

  document.addEventListener("click", (e) => {
    if (!root.contains(e.target)) close();
  });

  accentInput.oninput = () => {
    saveThemeSettings({ customAccent: accentInput.value });
    initTheme();
  };

  const settings = loadThemeSettings();
  accentInput.value = settings.customAccent;
  paint(settings.themeId);
}

export function wireSettingsPage() {
  wireDefaultPageDropdown();
  renderScreenList();
  wireThemeSection();
}
