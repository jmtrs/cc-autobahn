// CHECK ENGINE overlay (D9, Phase 4): without ccusage/npx/bunx in PATH there's
// no data. Same pattern as the sensor overlay: initial state via command
// (avoids racing against the event) + a button that triggers the install.

let engineInvoke = null;

const ENGINE_DEFAULT_BODY =
  "ccusage was not found (neither global, npx, nor bunx) in PATH.\n" +
  "Without an engine there is no usage data.";

export function showEngineOverlay(show) {
  document.getElementById("engine-overlay").hidden = !show;
  if (show) setEngineBody(ENGINE_DEFAULT_BODY); // reset after a previous error
}

function setEngineBody(text) {
  document.getElementById("engine-body").textContent = text;
}

async function onInstallEngineClick() {
  if (!engineInvoke) return;
  const btn = document.getElementById("engine-install-btn");
  if (btn.disabled) return; // double-click: installer already in progress
  btn.disabled = true;
  setEngineBody(
    "Installing Bun (curl -fsSL https://bun.sh/install | bash)…\nThis takes a few seconds."
  );
  try {
    const label = await engineInvoke("install_bun");
    setEngineBody(`Engine detected (${label}). Starting…`);
    showEngineOverlay(false); // blocks-update/engine-detected will confirm it shortly
  } catch (e) {
    setEngineBody(String(e));
    btn.disabled = false;
  }
}

/**
 * Wires the CHECK ENGINE overlay's initial state + install button.
 * Guarded: under plain `vite` (no Tauri) there is no IPC, so we skip silently.
 */
export async function wireEngineOverlay() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { invoke } = await import("@tauri-apps/api/core");
  engineInvoke = invoke;
  document.getElementById("engine-install-btn").onclick = onInstallEngineClick;
  try {
    const present = await engineInvoke("engine_status");
    showEngineOverlay(!present);
  } catch (e) {
    console.error("[engine] engine_status:", e);
  }
}
