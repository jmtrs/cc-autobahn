const issuesByProvider = new Map();

function render(provider, documentRoot = document) {
  const root = documentRoot.querySelector(`[data-provider-module="${provider}"]`);
  if (!root) return;
  const issues = [...(issuesByProvider.get(provider)?.values() ?? [])];
  const baseIssue = root.dataset.providerAvailable === "false" ? ["UNAVAILABLE"] : [];
  const allLabels = [...issues, ...baseIssue];
  const labels = allLabels.length > 1 ? [allLabels[0], `+${allLabels.length - 1}`] : allLabels;
  root.dataset.providerDegraded = String(allLabels.length > 0);
  root.setAttribute(
    "aria-label",
    [provider.toUpperCase(), ...allLabels].join(" · "),
  );
  root.querySelector(".provider-label").textContent = [provider.toUpperCase(), ...labels].join(" · ");
}

export function setProviderIssue(provider, key, label, active, documentRoot = document) {
  if (!issuesByProvider.has(provider)) issuesByProvider.set(provider, new Map());
  const issues = issuesByProvider.get(provider);
  if (active) issues.set(key, label);
  else issues.delete(key);
  render(provider, documentRoot);
}

export function setProviderAvailability(provider, available, documentRoot = document) {
  const root = documentRoot.querySelector(`[data-provider-module="${provider}"]`);
  if (!root) return false;
  root.dataset.providerAvailable = String(Boolean(available));
  render(provider, documentRoot);
  return true;
}
