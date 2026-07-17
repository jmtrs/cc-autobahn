// Shared mutable state between trip-computer (writer of block/sensor data),
// speedometer (writer of burn ticks), and footer-metric (reader of both, for
// the PACE/AUTO computation). A single shared object avoids a circular
// import between trip-computer.js and footer-metric.js.

export const state = {
  lastBlock: null, // last blocks-update, to re-apply the estimate on disconnect
  // `sensorConnected` = is the sensor pushing FRESH data right now? (D-review:
  // a normal pause with no Claude Code rendering sets it to false for a few
  // seconds, without being a real "never connected"). `everSensorConnected` is
  // sticky — once true, it never falls back to ccusage's projection again
  // (independent 5h window system; the jump between the two looked
  // absurd: official "0h17" → ccusage's "EST 4h31").
  sensorConnected: false, // is official data arriving from the statusLine?
  everSensorConnected: false, // did it ever connect? (sticky, see above)
  // `everQuotaConnected` is narrower than `everSensorConnected`: the statusLine
  // file can connect (and stay connected) without ever carrying `rate_limits`
  // (non-Pro/Max subscriber — see sensor::mod.rs's `tolerates_missing_rate_limits`).
  // Quota-based gauges (segments/autonomie text, D40) must gate on THIS flag,
  // not `everSensorConnected` — gating on the generic one made non-Pro/Max
  // users' gauge freeze on a fabricated "100%" forever, since fiveHourPct
  // never gets a real value to replace its 0 default (found in review).
  everQuotaConnected: false, // did a payload with a real `fiveHourPct` ever arrive? (sticky)
  fiveHourResetsAtMs: 0, // epoch-ms of the 5h reset (fallback countdown, estimated-only)
  fiveHourPct: 0, // official used_percentage of the 5h quota, re-read by the clock tick
  sevenDayPct: 0, // official 7d rate-limit window used%, read by limits-page (Page 2)
  sevenDayResetsAtMs: 0, // epoch-ms of the 7d reset, read by limits-page (Page 2)
  recentTicks: [], // { recvAt, tokens } — fed by onBurnTick, read by footer-metric's PACE
  recentPct: [], // { recvAt, pct } — fed by onSensorUpdate, read by footer-metric's AUTO
};
