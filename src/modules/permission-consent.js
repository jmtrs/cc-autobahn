// Permission hook consent UI (D42): connect/disconnect the PermissionRequest
// hook. Mutates ~/.claude/settings.json from the backend; unlike the
// statusLine sensor (sensor-consent.js) this overlay is opt-in only — opened
// from a Settings-page button, never auto-shown at startup, since the hook
// is an optional extra capability, not something the app's core value
// depends on.

let permissionInvoke = null;
let permissionInstalled = false;

export async function wirePermissionConsent() {
  if (!("__TAURI_INTERNALS__" in window)) return; // outside Tauri, nothing to do
  const { invoke } = await import("@tauri-apps/api/core");
  permissionInvoke = invoke;

  document.getElementById("permission-settings-btn").onclick = openOverlay;
  document.getElementById("permission-consent-connect").onclick = onConnectClick;
  document.getElementById("permission-consent-disconnect").onclick = onDisconnectClick;
  document.getElementById("permission-consent-cancel").onclick = closeOverlay;

  await refreshPermissionStatus();
}

async function refreshPermissionStatus() {
  if (!permissionInvoke) return;
  try {
    const st = await permissionInvoke("permission_status");
    permissionInstalled = !!st.installed;
    updateSettingsButtonLabel();
  } catch (e) {
    console.error("[permission] status:", e);
  }
}

function updateSettingsButtonLabel() {
  document.getElementById("permission-settings-btn").textContent = permissionInstalled
    ? "Manage"
    : "Connect";
}

function openOverlay() {
  document.getElementById("permission-consent-overlay").hidden = false;
  document.getElementById("permission-consent-disconnect").hidden = !permissionInstalled;
  const connect = document.getElementById("permission-consent-connect");
  connect.hidden = permissionInstalled;
  connect.textContent = "Connect";
  connect.onclick = onConnectClick;
  document.getElementById("permission-consent-cancel").hidden = false;
  setBody(
    permissionInstalled
      ? "The PermissionRequest hook is connected. Any Claude Code session's permission prompts show up here for Approve/Deny."
      : "Connect a PermissionRequest hook so Claude Code sessions ask for permission directly in this window instead of the terminal.\nModifies ~/.claude/settings.json with backup and rollback.\nYour other hooks are preserved."
  );
}

function closeOverlay() {
  document.getElementById("permission-consent-overlay").hidden = true;
}

function setBody(text) {
  document.getElementById("permission-consent-body").textContent = text;
}

async function onConnectClick() {
  if (!permissionInvoke) return;
  try {
    const p = await permissionInvoke("permission_preview_install");
    const preserved =
      p.existingHookCount > 0
        ? `Your ${p.existingHookCount} existing permission hook(s) are preserved.`
        : "You have no other permission hooks configured.";
    setBody(
      `A PermissionRequest hook (matcher "*", all tools) will be added to settings.json.\n${preserved}\nBackup: ${p.backupPath}\n\n` +
        "If cc-autobahn isn't running, Claude Code falls back to its normal terminal prompt automatically.\n" +
        "If something goes wrong: remove the hook from settings.json or restore the backup."
    );
    const connect = document.getElementById("permission-consent-connect");
    connect.hidden = false;
    connect.textContent = "Confirm";
    connect.onclick = doInstall;
  } catch (e) {
    setBody("Could not generate the preview: " + e);
  }
}

async function doInstall() {
  try {
    await permissionInvoke("install_permission_hook");
    await refreshPermissionStatus();
    closeOverlay();
  } catch (e) {
    setBody("Install error: " + e + "\n(settings untouched — automatic rollback)");
  }
}

async function onDisconnectClick() {
  try {
    await permissionInvoke("uninstall_permission_hook");
    await refreshPermissionStatus();
    closeOverlay();
  } catch (e) {
    setBody("Disconnect error: " + e);
  }
}
