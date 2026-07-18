// D41: move the panel with a plain click-and-drag, restricted to two zones
// (D-review, so it doesn't fight the dense instrument readouts elsewhere):
// the header row (nameplate/page label/MFD/PIN strip) and the PRND gear
// selector. Same semantics as native `data-tauri-drag-region` — applied via
// JS instead because the window's own bezel is near-zero (D-review,
// edge-to-edge look) and too thin to grab natively. Elements with their own
// click behavior inside those zones — the MFD/PIN buttons and `#nameplate`
// (marked `data-no-drag`) — are excluded so a normal click there still
// works instead of moving the window.

import { hintOnHover } from "./header-hint.js";

const DRAG_ZONE_SELECTOR = ".row.header, .gear";
const NO_DRAG_SELECTOR = "button, [data-no-drag]";

export async function wireWindowDrag() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { getCurrentWindow } = await import("@tauri-apps/api/window");
  const win = getCurrentWindow();

  document.addEventListener("mousedown", (e) => {
    if (e.button !== 0) return;
    if (!e.target.closest(DRAG_ZONE_SELECTOR)) return;
    if (e.target.closest(NO_DRAG_SELECTOR)) return;
    e.preventDefault();
    win.startDragging();
  });
}

/** Settings page (Page 3) button: undoes a manual drag without needing the
 *  tray's right-click menu. Takes effect immediately (backend re-anchors
 *  under the tray icon right away if the panel is visible, D-review). */
export async function wireResetPositionButton() {
  const btn = document.getElementById("reset-position-btn");
  if (!btn) return;
  hintOnHover(btn, "Move the panel back under the tray icon");
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { invoke } = await import("@tauri-apps/api/core");
  btn.onclick = () => {
    invoke("reset_position").catch((e) =>
      console.error("[window-drag] reset_position:", e)
    );
  };
}
