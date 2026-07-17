// PIN button (D24): pins the panel open despite losing focus. The hide-on-blur
// logic lives in Rust (window.rs); here we just report the state via `set_pinned`.

import { hintOnHover } from "./header-hint.js";

export async function wirePinButton() {
  const btn = document.getElementById("pin-btn");
  // Hint wired unconditionally (D-review): it's plain UI, no reason to gate
  // it behind the Tauri guard below like the actual pin functionality.
  hintOnHover(btn, "Keep panel open");
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { invoke } = await import("@tauri-apps/api/core");
  let pinned = false;
  btn.onclick = () => {
    pinned = !pinned;
    btn.classList.toggle("on", pinned);
    invoke("set_pinned", { value: pinned }).catch((e) =>
      console.error("[pin] set_pinned:", e)
    );
  };
}
