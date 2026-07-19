// CHECK ENGINE overlay (D9, Phase 4): without ccusage/npx/bunx in PATH there's
// no data. Same pattern as the sensor overlay: initial state via command
// (avoids racing against the event) + a button that triggers the install.

let engineInvoke = null;

const ENGINE_DEFAULT_BODY =
  "ccusage was not found (neither global, npx, nor bunx) in PATH.\n" +
  "Without an engine there is no usage data.";

// Stage text for the "install-progress" event (D36): the Bun installer is a
// single blocking `curl | bash`, no fractional progress to read — these are
// the only checkpoints Rust can report.
const INSTALL_STAGE_TEXT = {
  downloading: "Downloading & installing Bun (curl -fsSL https://bun.sh/install | bash)…",
  detecting: "Bun installed. Detecting ccusage…",
};

// Short label mirrored, the button itself is the most-glanced-at element
// during install — the body text/spinner are easy to miss in a compact panel.
const INSTALL_STAGE_BTN_LABEL = {
  downloading: "Downloading…",
  detecting: "Detecting…",
};

export function showEngineOverlay(show) {
  document.getElementById("engine-overlay").hidden = !show;
  if (show) {
    setEngineBody(ENGINE_DEFAULT_BODY); // reset after a previous error
    resetInstallButton();
  }
}

function setEngineBody(text) {
  document.getElementById("engine-body").textContent = text;
}

function setSpinner(show) {
  document.getElementById("engine-spinner").hidden = !show;
}

function resetInstallButton() {
  setSpinner(false);
  const btn = document.getElementById("engine-install-btn");
  btn.disabled = false;
  btn.textContent = "Install engine";
}

export function onInstallProgress(stage) {
  const text = INSTALL_STAGE_TEXT[stage];
  if (text) setEngineBody(text);
  const label = INSTALL_STAGE_BTN_LABEL[stage];
  if (label) document.getElementById("engine-install-btn").textContent = label;
}

// install_bun is fire-and-forget (D36-review: a blocking command freezes the
// whole webview, button label included — see engine/install.rs). The outcome
// arrives later via "install-succeeded"/"install-failed", not the invoke's
// return value.
export function onInstallSucceeded(label) {
  setEngineBody(`Engine detected (${label}). Starting…`);
  showEngineOverlay(false); // blocks-update/app-engine-detected will confirm it shortly
}

export function onInstallFailed(message) {
  setEngineBody(message);
  resetInstallButton();
}

async function onInstallEngineClick() {
  if (!engineInvoke) return;
  const btn = document.getElementById("engine-install-btn");
  if (btn.disabled) return; // double-click: installer already in progress
  btn.disabled = true;
  btn.textContent = INSTALL_STAGE_BTN_LABEL.downloading;
  setSpinner(true);
  setEngineBody(INSTALL_STAGE_TEXT.downloading);
  try {
    await engineInvoke("install_bun"); // just kicks off the background thread
  } catch (e) {
    setEngineBody(String(e));
    resetInstallButton();
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
