// VFD-style number formatters, shared across the dashboard widgets.

/** Formats tok/s VFD-style: "7.2", "55", "1.5k". */
export function formatTps(tps) {
  if (tps < 0.5) return "0";
  if (tps < 10) return tps.toFixed(1);
  if (tps < 1000) return Math.round(tps).toString();
  return (tps / 1000).toFixed(1) + "k";
}

/** Formats tokens VFD-style: "999", "1.5k", "850k", "1.24M", "2.1G". */
export function formatTokens(n) {
  if (!(n >= 1)) return "0";
  if (n < 1e3) return String(Math.round(n));
  if (n < 1e6) return (n / 1e3).toFixed(n < 1e5 ? 1 : 0).replace(/\.0$/, "") + "k";
  if (n < 1e9) return (n / 1e6).toFixed(2) + "M";
  return (n / 1e9).toFixed(2) + "G";
}

/** Remaining minutes → "3h12" (window autonomy). */
export function formatHMin(minutes) {
  if (!(minutes >= 0)) return "—";
  // Round ONCE to a whole minute before splitting into h/m: rounding each
  // part separately can produce m=60 (e.g. 119.5 → h=1, round(59.5)=60 →
  // "1h60" instead of "2h00"; bug found while reviewing the formulas).
  const total = Math.round(minutes);
  const h = Math.floor(total / 60);
  const m = total % 60;
  return `${h}h${String(m).padStart(2, "0")}`;
}

/** duration in ms → "H:MM" (time elapsed since the block started). */
export function formatDurationMs(ms) {
  if (!(ms > 0)) return "0:00";
  const totalMin = Math.floor(ms / 60000);
  return `${Math.floor(totalMin / 60)}:${String(totalMin % 60).padStart(2, "0")}`;
}
