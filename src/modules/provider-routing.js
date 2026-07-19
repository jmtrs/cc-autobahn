import { providerIdFromPayload, state, updateProviderHealth } from "./telemetry-state.js";

export function routeClaudePayload(payload, handler, channel = null, rejectEqual = false) {
  if (providerIdFromPayload(payload) !== "claude") return false;
  return routeProviderPayload(payload, handler, channel, rejectEqual);
}

export function routeProviderPayload(payload, handler, channel = null, rejectEqual = false) {
  const provider = providerIdFromPayload(payload);
  if (!provider) return false;
  if (channel) {
    const observedAtMs = Number(payload.observedAtMs);
    if (!Number.isFinite(observedAtMs) || observedAtMs < 0) return false;
    const current = state.providers[provider].lastEventAtMs[channel] ?? -1;
    if (observedAtMs < current || (rejectEqual && observedAtMs === current)) return false;
    state.providers[provider].lastEventAtMs[channel] = observedAtMs;
  }
  handler(payload);
  return true;
}

export function hydrateProviderHealth(snapshot) {
  if (!Array.isArray(snapshot)) return 0;
  return snapshot.reduce((count, health) => count + Number(updateProviderHealth(health)), 0);
}
