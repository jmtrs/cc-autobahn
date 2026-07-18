// cc-autobahn — frontend shell.
// Paints the W203 cluster and listens to the backend sensors (engine.rs + burn.rs).
// Clock + segment bar are static; speedometer = tok/s per response with a
// physical spring (D8): jumps when a turn completes and decays to idle. It is
// NOT instantaneous — the JSONL only reports usage when the turn closes (see D8/D11).

import { tickClock } from "./modules/clock.js";
import { wireCursor } from "./modules/cursor.js";
import { wireEngineOverlay } from "./modules/engine-overlay.js";
import { renderFooterMetric, wireFooterToggle } from "./modules/footer-metric.js";
import { wireHistoryPage } from "./modules/history-page.js";
import { wireEngine } from "./modules/ipc-events.js";
import { wireLimitsPage } from "./modules/limits-page.js";
import { wireMfdNav } from "./modules/mfd-nav.js";
import { wirePinButton } from "./modules/pin-button.js";
import { wireRedlineTray } from "./modules/redline.js";
import { wireSensorUi } from "./modules/sensor-consent.js";
import { wireSettingsPage } from "./modules/settings-page.js";
import { burnFrame } from "./modules/speedometer.js";
import { initTheme } from "./modules/theme.js";
import {
  buildSegments,
  setGear,
  wireNameplateEdit,
  wireTripComputerHints,
} from "./modules/trip-computer.js";

function init() {
  initTheme();
  wireCursor();
  // Autonomy bar empty until the first blocks-update (no data yet).
  buildSegments(0);
  tickClock();
  setInterval(tickClock, 1000);
  wireEngineOverlay();
  wireEngine();
  wireSensorUi();
  wirePinButton();
  wireRedlineTray();
  wireFooterToggle();
  wireNameplateEdit();
  wireTripComputerHints();
  // Page listeners wired before wireMfdNav() so its initial activate() (which
  // may land on Page 1/2 if that's the saved default) is already observed.
  wireHistoryPage();
  wireLimitsPage();
  wireSettingsPage();
  wireMfdNav();
  renderFooterMetric();
  setGear(["opus"]); // positions the marker against the HTML's default gear
  requestAnimationFrame(burnFrame); // starts idle (pos=0), true to the car
}

window.addEventListener("DOMContentLoaded", init);
