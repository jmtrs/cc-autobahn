// Wires the backend engine/burn/sensor events (see src-tauri/src/engine, burn,
// sensor) into the widget modules. Guarded: under plain `vite` (no Tauri)
// there is no IPC, so we skip silently.

import {
  onInstallFailed,
  onInstallProgress,
  onInstallSucceeded,
  showEngineOverlay,
} from "./engine-overlay.js";
import { onPermissionPending, onPermissionResolved } from "./permission-gate.js";
import { onBurnTick } from "./speedometer.js";
import { onBlocksUpdate, onSensorState, onSensorUpdate } from "./trip-computer.js";
import { claudeView } from "./provider-view.js";
import { hydrateProviderHealth, routeClaudePayload } from "./provider-routing.js";
import { updateProviderHealth } from "./telemetry-state.js";

export async function wireEngine() {
  if (!("__TAURI_INTERNALS__" in window)) return; // running outside Tauri
  const { listen } = await import("@tauri-apps/api/event");
  const { invoke } = await import("@tauri-apps/api/core");

  await listen("provider-health", (e) => {
    if (updateProviderHealth(e.payload)) {
      console.info("[provider] health:", e.payload);
    }
  });
  await listen("app-engine-detected", () => showEngineOverlay(false));
  await listen("app-engine-missing", () => showEngineOverlay(true));
  await listen("install-progress", (e) => onInstallProgress(e.payload));
  await listen("install-succeeded", (e) => onInstallSucceeded(e.payload));
  await listen("install-failed", (e) => onInstallFailed(e.payload));
  await listen("app-engine-error", (e) => console.error("[engine] error:", e.payload));
  await listen("blocks-idle", (e) => {
    routeClaudePayload(e.payload, () => console.info("[engine] no active Claude block"));
  });
  await listen("blocks-update", (e) => {
    routeClaudePayload(e.payload, (payload) => {
      console.info("[engine] blocks-update:", payload);
      showEngineOverlay(false);
      onBlocksUpdate(payload, claudeView);
    });
  });
  await listen("burn-tick", (e) => {
    routeClaudePayload(e.payload, (payload) => {
      console.info("[burn] tok/s per response:", payload);
      onBurnTick(payload, claudeView);
    });
  });
  await listen("sensor-update", (e) => {
    routeClaudePayload(e.payload, (payload) => {
      console.info("[sensor] official:", payload);
      onSensorUpdate(payload, claudeView);
    }, "sensor-update");
  });
  await listen("sensor-state", (e) => {
    routeClaudePayload(e.payload, (payload) => {
      console.info("[sensor] state:", payload);
      onSensorState(payload, claudeView);
    }, "sensor-state");
  });
  await listen("permission-pending", (e) => {
    routeClaudePayload(e.payload, (payload) => {
      console.info("[permission] pending:", payload);
      onPermissionPending(payload);
    });
  });
  await listen("permission-resolved", (e) => {
    routeClaudePayload(e.payload, () => {
      console.info("[permission] resolved");
      onPermissionResolved();
    });
  });

  try {
    hydrateProviderHealth(await invoke("provider_health_snapshot"));
  } catch (e) {
    console.error("[provider] health snapshot:", e);
  }
  try {
    const snapshot = await invoke("sensor_snapshot");
    if (snapshot?.update) {
      routeClaudePayload(
        snapshot.update,
        (payload) => onSensorUpdate(payload, claudeView),
        "sensor-update",
        true,
      );
    }
    if (snapshot?.state) {
      routeClaudePayload(
        snapshot.state,
        (payload) => onSensorState(payload, claudeView),
        "sensor-state",
        true,
      );
    }
  } catch (e) {
    console.error("[sensor] snapshot:", e);
  }
}
