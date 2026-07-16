// Trip-computer clock, like the W203 bottom-right time.

import { renderFooterMetric } from "./footer-metric.js";
import { refreshAutonomie } from "./trip-computer.js";

/** Tick the trip-computer clock. */
export function tickClock() {
  const el = document.getElementById("clock");
  const now = new Date();
  const hh = String(now.getHours()).padStart(2, "0");
  const mm = String(now.getMinutes()).padStart(2, "0");
  el.textContent = `${hh}:${mm}`;
  // Keeps counting even while the sensor is momentarily silent — the known
  // reset (fiveHourResetsAtMs) is still valid regardless (see D-review).
  refreshAutonomie();
  renderFooterMetric(); // trims the PACE/AUTO buffers even if no new event arrives
}
