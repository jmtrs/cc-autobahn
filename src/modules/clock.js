// Trip-computer clock, like the W203 bottom-right time.

import { renderFooterMetric } from "./footer-metric.js";
import { claudeView } from "./provider-view.js";
import { refreshAutonomie } from "./trip-computer.js";

/** Tick the one chassis clock, then refresh each provider's derived fields. */
export function tickClock(views = [claudeView]) {
  const list = Array.isArray(views) ? views : [views];
  const el = list[0].chassisElement("clock");
  const now = new Date();
  const hh = String(now.getHours()).padStart(2, "0");
  const mm = String(now.getMinutes()).padStart(2, "0");
  el.textContent = `${hh}:${mm}`;
  // Keeps counting even while the sensor is momentarily silent — the known
  // reset (fiveHourResetsAtMs) is still valid regardless (see D-review).
  list.forEach((view) => {
    refreshAutonomie(view);
    renderFooterMetric(view); // trims provider buffers even without new events
  });
}
