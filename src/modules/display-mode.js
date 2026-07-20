import { loadDisplayMode, saveDisplayMode } from "./app-settings.js";
import { setDisplayModeState, state } from "./telemetry-state.js";

export const DISPLAY_MODES = Object.freeze(["claude", "codex", "both"]);

function validMode(mode) {
  if (!DISPLAY_MODES.includes(mode)) throw new Error(`invalid display mode: ${mode}`);
  return mode;
}

function nameplateProvider(mode) {
  if (mode !== "both") return mode;
  return state.global.lastActiveModel?.provider ?? "claude";
}

export function paintDisplayMode(mode, documentRoot = document) {
  validMode(mode);
  const chassis = documentRoot.querySelector("[data-app-chassis]");
  if (!chassis) throw new Error("app chassis is unavailable");
  chassis.dataset.displayMode = mode;
  documentRoot.querySelectorAll("[data-provider-module]").forEach((root) => {
    root.hidden = mode !== "both" && root.dataset.providerModule !== mode;
  });
  setDisplayModeState(mode);

  const provider = nameplateProvider(mode);
  const nameplate = chassis.querySelector('[data-chassis-role="nameplate"]');
  if (nameplate && nameplate.contentEditable !== "true") {
    nameplate.textContent = state.providers[provider].nameplateLabel ?? "—";
  }
  documentRoot.dispatchEvent?.(
    new CustomEvent("display-mode-changed", { detail: { mode } }),
  );
  return mode;
}

async function nativeInvoke() {
  if (!("__TAURI_INTERNALS__" in window)) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke;
}

/** Native resize succeeds before UI/storage change, preventing divergence. */
export async function changeDisplayMode(
  mode,
  {
    documentRoot = document,
    invoke = undefined,
    persist = saveDisplayMode,
  } = {},
) {
  validMode(mode);
  const previous = state.global.displayMode;
  const invokeCommand = invoke === undefined ? await nativeInvoke() : invoke;
  if (invokeCommand) await invokeCommand("set_display_mode", { mode });
  let persisted = false;
  try {
    persist(mode);
    persisted = true;
    paintDisplayMode(mode, documentRoot);
  } catch (error) {
    if (persisted && previous !== mode) {
      try {
        persist(previous);
      } catch (rollbackError) {
        console.error("[display-mode] storage rollback:", rollbackError);
      }
    }
    if (invokeCommand && previous !== mode) {
      try {
        await invokeCommand("set_display_mode", { mode: previous });
      } catch (rollbackError) {
        console.error("[display-mode] native rollback:", rollbackError);
      }
    }
    throw error;
  }
  return mode;
}

/** Applies saved mode at startup; native failure safely paints Claude only. */
export async function initializeDisplayMode(
  {
    documentRoot = document,
    load = loadDisplayMode,
    invoke = undefined,
  } = {},
) {
  const mode = validMode(load());
  const invokeCommand = invoke === undefined ? await nativeInvoke() : invoke;
  if (invokeCommand) {
    try {
      await invokeCommand("set_display_mode", { mode });
    } catch (error) {
      paintDisplayMode("claude", documentRoot);
      throw error;
    }
  }
  return paintDisplayMode(mode, documentRoot);
}
