// Sensor consent UI (D12): connect/disconnect the statusLine. Mutates
// ~/.claude/settings.json from the backend; the overlay asks for confirmation
// with a preview (backup + chain) before writing.

let sensorInvoke = null;
let sensorInstalled = false;

/**
 * Wires the sensor-consent overlay (connect/disconnect/cancel) and refreshes
 * its initial state. Guarded: no-op outside Tauri.
 */
export async function wireSensorUi() {
  if (!("__TAURI_INTERNALS__" in window)) return; // outside Tauri, nothing to do
  const { invoke } = await import("@tauri-apps/api/core");
  sensorInvoke = invoke;

  document.getElementById("sensor-connect").onclick = onConnectClick;
  document.getElementById("sensor-disconnect").onclick = onDisconnectClick;
  document.getElementById("sensor-cancel").onclick = cancelPreview;

  refreshSensorStatus();
}

async function refreshSensorStatus() {
  if (!sensorInvoke) return;
  try {
    const st = await sensorInvoke("sensor_status");
    sensorInstalled = !!st.installed;
    document.getElementById("sensor-disconnect").hidden = !sensorInstalled;
    showSensorOverlay(!sensorInstalled);
  } catch (e) {
    console.error("[sensor] status:", e);
  }
}

function showSensorOverlay(show) {
  document.getElementById("sensor-overlay").hidden = !show;
  if (!show) return;
  setSensorBody(
    "Connect the sensor for the official rate_limits (5h / 7d window).\n" +
      "Modifies ~/.claude/settings.json with backup and rollback.\n" +
      "Your current statusLine is preserved (chain)."
  );
  const connect = document.getElementById("sensor-connect");
  connect.textContent = "Connect";
  connect.onclick = onConnectClick;
  document.getElementById("sensor-cancel").hidden = true;
}

function setSensorBody(text) {
  document.getElementById("sensor-body").textContent = text;
}

async function onConnectClick() {
  if (!sensorInvoke) return;
  try {
    const p = await sensorInvoke("sensor_preview_install");
    const prev = p.prevStatusLine
      ? "Your current statusLine is preserved and will keep rendering (chain)."
      : "You have no previous statusLine; the sensor will use a default line.";
    setSensorBody(
      `statusLine will be written to settings.json.\n${prev}\nBackup: ${p.backupPath}\n\n` +
        "If something goes wrong: delete statusLine or restore the backup."
    );
    const connect = document.getElementById("sensor-connect");
    connect.textContent = "Confirm";
    connect.onclick = doInstall;
    document.getElementById("sensor-cancel").hidden = false;
  } catch (e) {
    setSensorBody("Could not generate the preview: " + e);
  }
}

async function doInstall() {
  try {
    await sensorInvoke("install_sensor");
    refreshSensorStatus();
  } catch (e) {
    setSensorBody("Install error: " + e + "\n(settings untouched — automatic rollback)");
  }
}

function cancelPreview() {
  showSensorOverlay(!sensorInstalled);
}

async function onDisconnectClick() {
  try {
    await sensorInvoke("uninstall_sensor");
    refreshSensorStatus();
  } catch (e) {
    setSensorBody("Disconnect error: " + e);
  }
}
