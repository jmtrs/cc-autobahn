// On-demand fetch of ccusage's daily history — shared by Page 1 (bars) and
// Page 2 (today's per-model breakdown), so both pages spend a single
// ccusage process spawn instead of two. Deliberately NOT part of the
// continuous engine poll (D13): daily totals barely move within a day, so
// this only runs when a page that needs it is opened, cached briefly to
// avoid re-spawning ccusage on every page flip.

const CACHE_MS = 5 * 60 * 1000;
const caches = new Map(); // provider -> { at, days }

// Shared loading indicator for the first (uncached) `loadHistory()` call —
// reuses the CHECK ENGINE overlay's "VFD scanner" (.engine-spinner, D36)
// instead of inventing a second spinner, so a slow ccusage spawn doesn't
// read as a frozen page to whoever opens History/Limits first.
export const SPINNER_HTML =
  '<div class="engine-spinner" aria-hidden="true">' +
  '<span class="engine-spinner-seg"></span>'.repeat(5) +
  "</div>";

/** `Array<{date, totalCost, totalTokens, modelBreakdowns}>`, oldest first. */
export async function loadHistory(provider = "claude", force = false) {
  const cache = caches.get(provider) ?? null;
  if (!force && cache && Date.now() - cache.at < CACHE_MS) return cache.days;
  if (!("__TAURI_INTERNALS__" in window)) return cache?.days ?? [];
  if (provider !== "claude") return [];
  const { invoke } = await import("@tauri-apps/api/core");
  const days = await invoke("history_daily");
  caches.set(provider, { at: Date.now(), days });
  return days;
}

/** Most recent day in a `loadHistory()` result (lexicographic ISO dates sort correctly). */
export function latestDay(days) {
  return days.reduce((latest, d) => (!latest || d.date > latest.date ? d : latest), null);
}
