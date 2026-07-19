// Provider-native PermissionRequest hook consent. Claude mutates its merged
// settings.json entry; Codex mutates user hooks.json and then leaves trust to
// Codex's own /hooks review flow.

const PROVIDERS = {
  claude: {
    label: "Claude",
    buttonId: "permission-settings-btn",
    status: "permission_status",
    preview: "permission_preview_install",
    install: "install_permission_hook",
    uninstall: "uninstall_permission_hook",
  },
  codex: {
    label: "Codex",
    buttonId: "codex-permission-settings-btn",
    status: "codex_permission_status",
    preview: "codex_permission_preview_install",
    install: "install_codex_permission_hook",
    uninstall: "uninstall_codex_permission_hook",
  },
};

let permissionInvoke = null;
let currentProvider = "claude";
let overlayGeneration = 0;
const statuses = { claude: null, codex: null };

export async function wirePermissionConsent() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { invoke } = await import("@tauri-apps/api/core");
  permissionInvoke = invoke;

  Object.entries(PROVIDERS).forEach(([provider, config]) => {
    document.getElementById(config.buttonId).onclick = async () => {
      const generation = ++overlayGeneration;
      await refreshPermissionStatus(provider);
      if (generation !== overlayGeneration) return;
      openOverlay(provider);
    };
  });
  document.getElementById("permission-consent-connect").onclick = onConnectClick;
  document.getElementById("permission-consent-disconnect").onclick = onDisconnectClick;
  document.getElementById("permission-consent-cancel").onclick = closeOverlay;

  await Promise.all(Object.keys(PROVIDERS).map(refreshPermissionStatus));
}

async function refreshPermissionStatus(provider) {
  if (!permissionInvoke) return;
  try {
    statuses[provider] = await permissionInvoke(PROVIDERS[provider].status);
    updateSettingsButtonLabel(provider);
  } catch (error) {
    statuses[provider] = null;
    updateSettingsButtonLabel(provider);
    console.error(`[permission] ${provider} status:`, error);
  }
}

export function permissionStatusLabel(provider, status) {
  if (!status?.installed && !status?.configuredLocally) return "Connect";
  if (provider === "claude") return "Manage";
  if (status.enabled === false) return "Disabled";
  if (status.trustStatus && status.trustStatus !== "trusted") return "Await trust";
  if (status.active && status.enabled === true && status.trustStatus === "trusted") return "Active";
  if (status.enabled === true && status.trustStatus === "trusted") return "Ready";
  return "Disconnected";
}

function updateSettingsButtonLabel(provider) {
  document.getElementById(PROVIDERS[provider].buttonId).textContent = permissionStatusLabel(
    provider,
    statuses[provider],
  );
}

function openOverlay(provider) {
  overlayGeneration += 1;
  currentProvider = provider;
  const config = PROVIDERS[provider];
  const status = statuses[provider];
  const installed = !!status?.installed || !!status?.configuredLocally;
  document.getElementById("permission-consent-overlay").hidden = false;
  document.getElementById("permission-consent-title").textContent =
    `${config.label.toUpperCase()} PERMISSION HOOK`;
  document.getElementById("permission-consent-disconnect").hidden = !installed;
  const connect = document.getElementById("permission-consent-connect");
  connect.hidden = installed;
  connect.textContent = "Connect";
  connect.onclick = onConnectClick;
  document.getElementById("permission-consent-cancel").hidden = false;
  setBody(installed ? installedBody(provider, status) : disconnectedBody(provider));
}

function installedBody(provider, status) {
  if (provider === "claude") {
    return "Claude PermissionRequest hook is connected. Claude sessions can route approval prompts here.";
  }
  if (status.enabled === false) {
    return "Codex hook is installed but disabled. Re-enable it from /hooks in Codex.";
  }
  if (status.trustStatus && status.trustStatus !== "trusted") {
    return "Codex hook is installed and awaiting native trust review. Open /hooks in Codex, inspect the command, then trust it.";
  }
  if (status.active && status.enabled && status.trustStatus === "trusted") {
    return "Codex hook is installed, trusted, and a runtime exchange has been observed.";
  }
  if (status.enabled && status.trustStatus === "trusted") {
    return "Codex hook is installed and trusted. It becomes ACTIVE after the first permission request reaches cc-autobahn.";
  }
  return "Codex hook is installed, but hooks/list is currently unavailable. Configuration and runtime activity cannot be verified.";
}

function disconnectedBody(provider) {
  return provider === "claude"
    ? "Connect Claude PermissionRequest so approval prompts can appear in this window.\nModifies ~/.claude/settings.json with backup and rollback.\nOther hooks are preserved."
    : "Connect Codex PermissionRequest at the active user hooks.json layer.\nCodex will require native review and trust through /hooks before the command can run.\nOther hooks are preserved.";
}

function closeOverlay() {
  overlayGeneration += 1;
  document.getElementById("permission-consent-overlay").hidden = true;
}

function setBody(text) {
  document.getElementById("permission-consent-body").textContent = text;
}

async function onConnectClick() {
  if (!permissionInvoke) return;
  const provider = currentProvider;
  const generation = overlayGeneration;
  const config = PROVIDERS[provider];
  try {
    const preview = await permissionInvoke(config.preview);
    if (provider !== currentProvider || generation !== overlayGeneration) return;
    const preserved =
      preview.existingHookCount > 0
        ? `${preview.existingHookCount} existing permission hook(s) will be preserved.`
        : "No other permission hooks are configured in this layer.";
    const trust =
      provider === "codex"
        ? "\nAfter install: open /hooks in Codex and trust the exact command before it can run."
        : "";
    setBody(
      `A PermissionRequest hook (matcher "*", all supported tools) will be added.\n${preserved}\nBackup: ${preview.backupPath}${trust}\n\nIf cc-autobahn is unavailable, the provider keeps its native approval flow.`,
    );
    const connect = document.getElementById("permission-consent-connect");
    connect.hidden = false;
    connect.textContent = "Confirm";
    connect.onclick = () => doInstall(provider, generation);
  } catch (error) {
    if (provider !== currentProvider || generation !== overlayGeneration) return;
    setBody("Could not generate preview: " + error);
  }
}

async function doInstall(provider, generation) {
  if (provider !== currentProvider || generation !== overlayGeneration) return;
  try {
    await permissionInvoke(PROVIDERS[provider].install);
    if (provider !== currentProvider || generation !== overlayGeneration) return;
    await refreshPermissionStatus(provider);
    if (provider !== currentProvider || generation !== overlayGeneration) return;
    openOverlay(provider);
  } catch (error) {
    if (provider !== currentProvider || generation !== overlayGeneration) return;
    setBody("Install error: " + error + "\n(configuration rolled back when possible)");
  }
}

async function onDisconnectClick() {
  const provider = currentProvider;
  const generation = overlayGeneration;
  try {
    await permissionInvoke(PROVIDERS[provider].uninstall);
    if (provider !== currentProvider || generation !== overlayGeneration) return;
    await refreshPermissionStatus(provider);
    if (provider !== currentProvider || generation !== overlayGeneration) return;
    closeOverlay();
  } catch (error) {
    if (provider !== currentProvider || generation !== overlayGeneration) return;
    setBody("Disconnect error: " + error);
  }
}
