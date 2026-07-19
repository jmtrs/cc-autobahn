// Provider-isolated application state. Shared chassis state lives once under
// `global`; telemetry, health and future history caches never cross providers.

export const PROVIDERS = Object.freeze(["claude", "codex"]);
export const HEALTH_COMPONENTS = Object.freeze([
  "engine",
  "sensor",
  "history",
  "transcript",
  "permissions",
  "app-server",
]);
export const HEALTH_STATUSES = Object.freeze(["connected", "degraded", "unavailable"]);

export function createProviderState(provider) {
  if (!PROVIDERS.includes(provider)) throw new Error(`unknown provider: ${provider}`);
  return {
    provider,
    health: {},
    lastEventAtMs: {},
    lastBlock: null,
    sensorConnected: false,
    everSensorConnected: false,
    everQuotaConnected: false,
    autonomieShowTime: false,
    fiveHourResetsAtMs: 0,
    fiveHourPct: 0,
    sevenDayPct: 0,
    sevenDayResetsAtMs: 0,
    recentTicks: [],
    recentPct: [],
  };
}

export const state = {
  global: {
    displayMode: "claude",
    currentPage: 0,
    lastActiveModel: null,
    permissionHead: null,
  },
  providers: {
    claude: createProviderState("claude"),
    codex: createProviderState("codex"),
  },
};

// Existing renderers remain intentionally Claude-bound until Phase 2 creates
// provider-scoped DOM roots. This explicit alias prevents a Codex event from
// mutating the legacy singleton UI by accident.
export const claudeState = state.providers.claude;

export function providerIdFromPayload(payload) {
  return PROVIDERS.includes(payload?.provider) ? payload.provider : null;
}

export function setCurrentPage(page) {
  if (!Number.isInteger(page) || page < 0 || page > 3) return false;
  state.global.currentPage = page;
  return true;
}

export function setPermissionHead(payload) {
  state.global.permissionHead = payload ?? null;
}

export function updateProviderHealth(payload) {
  const provider = providerIdFromPayload(payload);
  if (
    !provider ||
    !HEALTH_COMPONENTS.includes(payload?.component) ||
    !HEALTH_STATUSES.includes(payload?.status)
  )
    return false;
  const observedAtMs = Number(payload.observedAtMs);
  if (!Number.isFinite(observedAtMs) || observedAtMs < 0) return false;
  const current = state.providers[provider].health[payload.component];
  if (current && observedAtMs < current.observedAtMs) return false;
  state.providers[provider].health[payload.component] = {
    status: payload.status,
    observedAtMs,
    detail: payload.detail ?? null,
  };
  return true;
}
