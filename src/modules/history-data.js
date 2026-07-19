// Provider-scoped on-demand ccusage reports. Daily data is shared by History
// and Limits; session data has the same cache boundary for future consumers.

import { formatUsd } from "./format.js";

const CACHE_MS = 5 * 60 * 1000;
const reportCaches = {
  daily: new Map(),
  session: new Map(),
};

export const SPINNER_HTML =
  '<div class="engine-spinner" aria-hidden="true">' +
  '<span class="engine-spinner-seg"></span>'.repeat(5) +
  "</div>";

function reportCache(kind, provider) {
  const caches = reportCaches[kind];
  if (!caches.has(provider)) caches.set(provider, { at: 0, rows: [], pending: null });
  return caches.get(provider);
}

async function nativeInvoke() {
  if (typeof window === "undefined" || !("__TAURI_INTERNALS__" in window)) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke;
}

async function loadReport(kind, provider, force, options) {
  if (!["claude", "codex"].includes(provider)) throw new Error(`unknown provider: ${provider}`);
  const cache = reportCache(kind, provider);
  const now = options.now?.() ?? Date.now();
  if (!force && cache.at > 0 && now - cache.at < CACHE_MS) return cache.rows;
  if (cache.pending) return cache.pending;

  const invoke = options.invoke ?? (await nativeInvoke());
  if (!invoke) return cache.rows;
  const command = kind === "daily" ? "history_daily" : "history_sessions";
  const pending = Promise.resolve(invoke(command, { provider }))
    .then((rows) => {
      if (!Array.isArray(rows)) throw new Error(`${command} returned a non-array payload`);
      if (rows.some((row) => row?.provider !== provider)) {
        throw new Error(`${command} returned a mismatched provider payload`);
      }
      cache.rows = rows;
      cache.at = options.now?.() ?? Date.now();
      return rows;
    })
    .finally(() => {
      if (cache.pending === pending) cache.pending = null;
    });
  cache.pending = pending;
  return pending;
}

/** `Array<{date, totalCost, totalTokens, modelBreakdowns}>`, oldest first. */
export function loadHistory(provider = "claude", force = false, options = {}) {
  return loadReport("daily", provider, force, options);
}

/** Provider-scoped session/thread report; raw local paths never cross IPC. */
export function loadSessionHistory(provider = "claude", force = false, options = {}) {
  return loadReport("session", provider, force, options);
}

export function clearHistoryCache(provider = null) {
  Object.values(reportCaches).forEach((caches) => {
    if (provider) caches.delete(provider);
    else caches.clear();
  });
}

export function formatHistoryCost(cost, provider) {
  if (cost == null || !Number.isFinite(Number(cost))) return "—";
  const formatted = formatUsd(cost);
  return provider === "codex" ? `EST ${formatted}` : formatted;
}

/** Most recent day in a result (lexicographic ISO dates sort correctly). */
export function latestDay(days) {
  return days.reduce((latest, day) => (!latest || day.date > latest.date ? day : latest), null);
}

export function localDateKey(now = new Date()) {
  return [
    now.getFullYear(),
    String(now.getMonth() + 1).padStart(2, "0"),
    String(now.getDate()).padStart(2, "0"),
  ].join("-");
}

export function todayEntry(days, now = new Date()) {
  const key = localDateKey(now);
  return days.find((day) => day.date === key) ?? null;
}
