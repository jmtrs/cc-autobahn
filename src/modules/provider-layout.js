const PROVIDER_META = Object.freeze({
  claude: { label: "CLAUDE", available: true },
  codex: { label: "CODEX · UNAVAILABLE", available: false },
});

function rewriteIdsAsRoles(root) {
  root.querySelectorAll("[id]").forEach((element) => {
    element.dataset.providerRole = element.id;
    element.removeAttribute("id");
  });
}

function resetCodexReadouts(root) {
  const text = {
    odo: "—",
    "session-time": "—",
    burn: "—",
    "footer-metric-value": "—",
    avg: "—",
    autonomie: "UNAVAILABLE",
    "history-total": "—",
    "hd-date": "no data source",
    "hd-total": "",
    "limit-pct": "—",
    "limit-reset": "no data source",
    "burn-instant": "—",
    "burn-avg": "—",
  };
  Object.entries(text).forEach(([role, value]) => {
    const element = root.querySelector(`[data-provider-role="${role}"]`);
    if (element) element.textContent = value;
  });
  const models = root.querySelectorAll(".gear .g");
  models.forEach((model, index) => {
    model.hidden = index !== 0;
    model.classList.toggle("active", index === 0);
    if (index === 0) {
      model.dataset.model = "unavailable";
      model.textContent = "?";
    }
  });
  root.querySelector('[data-provider-role="gear-marker"]')?.setAttribute("hidden", "");
}

function createModule(template, provider) {
  const fragment = template.content.cloneNode(true);
  const root = fragment.querySelector(".provider-module");
  root.dataset.providerModule = provider;
  root.dataset.providerAvailable = String(PROVIDER_META[provider].available);
  root.querySelector(".provider-label").textContent = PROVIDER_META[provider].label;
  rewriteIdsAsRoles(root);
  if (provider === "codex") resetCodexReadouts(root);
  return root;
}

/** Mounts both provider roots once. Display modes only hide/show them. */
export function mountProviderLayout(documentRoot = document) {
  const stack = documentRoot.getElementById("provider-stack");
  const template = documentRoot.getElementById("provider-module-template");
  if (!stack || !template) throw new Error("provider layout template is unavailable");
  if (stack.children.length > 0) return;
  stack.append(createModule(template, "claude"), createModule(template, "codex"));
}
