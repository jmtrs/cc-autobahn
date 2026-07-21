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
    rateLimitSourceQuality: "unavailable",
    rateLimitBuckets: [],
    accountUsage: null,
    autonomieShowTime: false,
    fiveHourResetsAtMs: 0,
    fiveHourPct: 0,
    primaryWindowDurationMinutes: 300,
    sevenDayPct: 0,
    sevenDayResetsAtMs: 0,
    secondaryWindowDurationMinutes: null,
    hasSecondaryLimit: false,
    recentTicks: [],
    activeSessionOrThreadId: null,
    sessionStartedAtMs: 0,
    lastTurnRateObservedAtMs: 0,
    recentPct: [],
    nameplateLabel: provider === "claude" ? "CC 500" : null,
    lastModelActivity: null,
  };
}

export function setDisplayModeState(displayMode) {
  if (!["claude", "codex", "both"].includes(displayMode)) return false;
  state.global.displayMode = displayMode;
  return true;
}

function isNewerModelActivity(activity, current) {
  if (!current) return true;
  if (activity.observedAtMs !== current.observedAtMs) {
    return activity.observedAtMs > current.observedAtMs;
  }
  if (
    activity.provider === current.provider &&
    activity.sessionOrThreadId === current.sessionOrThreadId
  ) {
    return activity.sequence > current.sequence;
  }
  const tieKey = (value) =>
    [value.provider, value.sessionOrThreadId ?? "", value.modelKey ?? "", value.label].join("\0");
  return tieKey(activity) > tieKey(current);
}

export function recordModelActivity(activity) {
  const provider = providerIdFromPayload(activity);
  const observedAtMs = Number(activity?.observedAtMs);
  const sequence = Number(activity?.sequence ?? 0);
  if (
    !provider ||
    !Number.isFinite(observedAtMs) ||
    observedAtMs < 0 ||
    !Number.isInteger(sequence) ||
    sequence < 0 ||
    typeof activity?.label !== "string" ||
    !activity.label
  ) {
    return { providerAccepted: false, globalAccepted: false };
  }
  const providerState = state.providers[provider];
  const providerCurrent = providerState.lastModelActivity;
  const candidate = {
    provider,
    modelKey: activity.modelKey ?? null,
    label: activity.label,
    sessionOrThreadId: activity.sessionOrThreadId ?? null,
    observedAtMs,
    sequence,
  };
  if (!isNewerModelActivity(candidate, providerCurrent)) {
    return { providerAccepted: false, globalAccepted: false };
  }
  const accepted = candidate;
  providerState.lastModelActivity = accepted;
  providerState.nameplateLabel = accepted.label;

  const current = state.global.lastActiveModel;
  if (!isNewerModelActivity(accepted, current)) {
    return { providerAccepted: true, globalAccepted: false };
  }
  state.global.lastActiveModel = accepted;
  return { providerAccepted: true, globalAccepted: true };
}

export function setLastActiveModel(activity) {
  return recordModelActivity(activity).globalAccepted;
}

export function reconcileNameplateEdit(provider, modelKey, label) {
  const providerState = state.providers[provider];
  if (!providerState) return null;
  const providerActivity = providerState.lastModelActivity;
  if (providerActivity?.modelKey === modelKey) {
    providerActivity.label = label;
    providerState.nameplateLabel = label;
  }
  const globalActivity = state.global.lastActiveModel;
  if (globalActivity?.provider === provider && globalActivity.modelKey === modelKey) {
    globalActivity.label = label;
  }
  return globalActivity?.label ?? label;
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

// Compatibility alias for non-rendering callers. Provider renderers resolve
// their own state through provider-view.js.
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
