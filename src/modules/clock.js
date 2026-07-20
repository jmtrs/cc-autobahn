// Periodic refresh for clock-derived provider metrics.

import { renderFooterMetric } from "./footer-metric.js";
import { claudeView } from "./provider-view.js";
import { refreshAutonomie, refreshSessionTime } from "./trip-computer.js";

/** Refresh provider fields whose values change as wall-clock time advances. */
export function tickClock(views = [claudeView]) {
  const list = Array.isArray(views) ? views : [views];
  // Keeps counting even while the sensor is momentarily silent — the known
  // reset (fiveHourResetsAtMs) is still valid regardless (see D-review).
  list.forEach((view) => {
    refreshAutonomie(view);
    refreshSessionTime(view);
    renderFooterMetric(view); // trims provider buffers even without new events
  });
}
