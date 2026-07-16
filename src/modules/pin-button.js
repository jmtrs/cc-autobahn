// PIN button (D24): pins the panel open despite losing focus. The hide-on-blur
// logic lives in Rust (window.rs); here we just report the state via `set_pinned`.

export async function wirePinButton() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { invoke } = await import("@tauri-apps/api/core");
  const btn = document.getElementById("pin-btn");
  let pinned = false;
  btn.onclick = () => {
    pinned = !pinned;
    btn.classList.toggle("on", pinned);
    invoke("set_pinned", { value: pinned }).catch((e) =>
      console.error("[pin] set_pinned:", e)
    );
  };
}
