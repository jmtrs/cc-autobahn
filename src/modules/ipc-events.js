// Wires the backend engine/burn/sensor events (see src-tauri/src/engine, burn,
// sensor) into the widget modules. Guarded: under plain `vite` (no Tauri)
// there is no IPC, so we skip silently.

import { showEngineOverlay } from "./engine-overlay.js";
import { onBurnTick } from "./speedometer.js";
import { onBlocksUpdate, onSensorState, onSensorUpdate } from "./trip-computer.js";

export async function wireEngine() {
  if (!("__TAURI_INTERNALS__" in window)) return; // running outside Tauri
  const { listen } = await import("@tauri-apps/api/event");

  listen("engine-detected", () => showEngineOverlay(false));
  listen("engine-missing", () => showEngineOverlay(true));
  listen("engine-error", (e) => console.error("[engine] error:", e.payload));
  listen("blocks-idle", () => console.info("[engine] no active block"));
  listen("blocks-update", (e) => {
    console.info("[engine] blocks-update:", e.payload);
    showEngineOverlay(false);
    onBlocksUpdate(e.payload);
  });
  listen("burn-tick", (e) => {
    console.info("[burn] tok/s per response:", e.payload);
    onBurnTick(e.payload);
  });
  listen("sensor-update", (e) => {
    console.info("[sensor] official:", e.payload);
    onSensorUpdate(e.payload);
  });
  listen("sensor-state", (e) => {
    console.info("[sensor] state:", e.payload);
    onSensorState(e.payload);
  });
}
