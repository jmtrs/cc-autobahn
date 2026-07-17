// On-demand fetch of ccusage's daily history — shared by Page 1 (bars) and
// Page 2 (today's per-model breakdown), so both pages spend a single
// ccusage process spawn instead of two. Deliberately NOT part of the
// continuous engine poll (D13): daily totals barely move within a day, so
// this only runs when a page that needs it is opened, cached briefly to
// avoid re-spawning ccusage on every page flip.

const CACHE_MS = 5 * 60 * 1000;
let cache = null; // { at, days }

/** `Array<{date, totalCost, totalTokens, modelBreakdowns}>`, oldest first. */
export async function loadHistory(force = false) {
  if (!force && cache && Date.now() - cache.at < CACHE_MS) return cache.days;
  if (!("__TAURI_INTERNALS__" in window)) return cache?.days ?? [];
  const { invoke } = await import("@tauri-apps/api/core");
  const days = await invoke("history_daily");
  cache = { at: Date.now(), days };
  return days;
}

/** Most recent day in a `loadHistory()` result (lexicographic ISO dates sort correctly). */
export function latestDay(days) {
  return days.reduce((latest, d) => (!latest || d.date > latest.date ? d : latest), null);
}
