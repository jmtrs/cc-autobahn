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
import { claudeView, codexView } from "./provider-view.js";
import { hydrateProviderHealth, routeClaudePayload } from "./provider-routing.js";
import { providerIdFromPayload, state, updateProviderHealth } from "./telemetry-state.js";
import { setProviderAvailability } from "./provider-status.js";
import { setGear } from "./trip-computer.js";

const providerViews = { claude: claudeView, codex: codexView };

function syncCodexAvailability() {
  const health = state.providers.codex.health;
  const available = ["transcript", "history"].some(
    (component) => health[component]?.status === "connected",
  );
  setProviderAvailability("codex", available);
}

function applyHealth(payload) {
  if (!updateProviderHealth(payload)) return false;
  if (payload.provider === "codex") syncCodexAvailability();
  return true;
}

export async function wireEngine() {
  if (!("__TAURI_INTERNALS__" in window)) return; // running outside Tauri
  const { listen } = await import("@tauri-apps/api/event");
  const { invoke } = await import("@tauri-apps/api/core");

  await listen("provider-health", (e) => {
    if (applyHealth(e.payload)) {
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
    const provider = providerIdFromPayload(e.payload);
    const view = providerViews[provider];
    if (!view) return;
    console.info("[burn] tok/s per response:", e.payload);
    onBurnTick(e.payload, view);
  });
  await listen("model-activity", (e) => {
    const provider = providerIdFromPayload(e.payload);
    const view = providerViews[provider];
    if (!view || typeof e.payload?.modelId !== "string") return;
    console.info("[provider] model activity:", e.payload);
    setGear([e.payload.modelId], view, {
      observedAtMs: e.payload.observedAtMs,
      sequence: e.payload.sequence,
      sessionOrThreadId: e.payload.sessionOrThreadId,
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
    const snapshot = await invoke("provider_health_snapshot");
    hydrateProviderHealth(snapshot);
    syncCodexAvailability();
  } catch (e) {
    console.error("[provider] health snapshot:", e);
  }
  try {
    const activities = await invoke("provider_activity_snapshot");
    activities?.forEach((activity) => {
      const view = providerViews[providerIdFromPayload(activity)];
      if (view && typeof activity?.modelId === "string") {
        setGear([activity.modelId], view, {
          observedAtMs: activity.observedAtMs,
          sequence: activity.sequence,
          sessionOrThreadId: activity.sessionOrThreadId,
        });
      }
    });
  } catch (e) {
    console.error("[provider] activity snapshot:", e);
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
