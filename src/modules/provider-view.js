import { state } from "./telemetry-state.js";

function requiredProviderState(provider) {
  const providerState = state.providers[provider];
  if (!providerState) throw new Error(`unknown provider view: ${provider}`);
  return providerState;
}

/**
 * Provider-local rendering boundary. Roots are resolved lazily because this
 * module is imported before DOMContentLoaded. `chassisElement` is reserved for
 * genuinely shared controls such as the dynamic nameplate.
 */
export function createProviderView({
  provider,
  documentRoot = globalThis.document,
  rootSelector = `[data-provider-module="${provider}"]`,
  chassisSelector = "[data-app-chassis]",
}) {
  const providerState = requiredProviderState(provider);

  function root() {
    const value = documentRoot?.querySelector(rootSelector);
    if (!value) throw new Error(`missing provider root: ${provider}`);
    return value;
  }

  function chassis() {
    return documentRoot?.querySelector(chassisSelector) ?? documentRoot;
  }

  return Object.freeze({
    provider,
    state: providerState,
    root,
    element(id) {
      return (
        root().querySelector(`[data-provider-role="${id}"]`) ??
        root().querySelector(`#${id}`)
      );
    },
    query(selector) {
      return root().querySelector(selector);
    },
    queryAll(selector) {
      return root().querySelectorAll(selector);
    },
    chassisElement(id) {
      return (
        chassis()?.querySelector(`[data-chassis-role="${id}"]`) ??
        chassis()?.querySelector(`#${id}`)
      );
    },
    emit(name) {
      documentRoot?.dispatchEvent(
        new CustomEvent(name, { detail: { provider } }),
      );
    },
  });
}

export const claudeView = createProviderView({ provider: "claude" });
export const codexView = createProviderView({ provider: "codex" });
