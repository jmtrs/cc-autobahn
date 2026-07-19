// cc-autobahn — frontend shell.
// Paints the W203 cluster and listens to the backend sensors (engine.rs + burn.rs).
// Clock + segment bar are static; speedometer = tok/s per response with a
// physical spring (D8): jumps when a turn completes and decays to idle. It is
// NOT instantaneous — the JSONL only reports usage when the turn closes (see D8/D11).

import { tickClock } from "./modules/clock.js";
import { wireCursor } from "./modules/cursor.js";
import { changeDisplayMode, initializeDisplayMode } from "./modules/display-mode.js";
import { wireEngineOverlay } from "./modules/engine-overlay.js";
import { renderFooterMetric, wireFooterToggle } from "./modules/footer-metric.js";
import { wireHistoryPage } from "./modules/history-page.js";
import { wireEngine } from "./modules/ipc-events.js";
import { wireLimitsPage } from "./modules/limits-page.js";
import { wireMfdNav } from "./modules/mfd-nav.js";
import { wirePermissionConsent } from "./modules/permission-consent.js";
import { wirePermissionGate } from "./modules/permission-gate.js";
import { wirePinButton } from "./modules/pin-button.js";
import { mountProviderLayout } from "./modules/provider-layout.js";
import { claudeView, codexView } from "./modules/provider-view.js";
import { wireRedlineTray } from "./modules/redline.js";
import { wireSensorUi } from "./modules/sensor-consent.js";
import { wireSettingsPage } from "./modules/settings-page.js";
import { startBurnAnimation } from "./modules/speedometer.js";
import { initTheme } from "./modules/theme.js";
import {
  buildSegments,
  setGear,
  wireNameplateEdit,
  wireTripComputerHints,
} from "./modules/trip-computer.js";
import { wireResetPositionButton, wireWindowDrag } from "./modules/window-drag.js";

async function init() {
  mountProviderLayout();
  try {
    await initializeDisplayMode();
  } catch (error) {
    console.error("[display-mode] startup transition:", error);
  }
  const providerViews = [claudeView, codexView];
  initTheme();
  wireCursor();
  providerViews.forEach((view) => buildSegments(0, view));
  tickClock(providerViews);
  setInterval(() => tickClock(providerViews), 1000);
  wireEngineOverlay();
  await wireEngine();
  wireSensorUi();
  await wirePermissionGate();
  wirePermissionConsent();
  wirePinButton();
  wireWindowDrag();
  wireResetPositionButton();
  wireRedlineTray();
  providerViews.forEach((view) => wireFooterToggle(view, providerViews));
  wireNameplateEdit(providerViews);
  wireTripComputerHints(claudeView);
  // Page listeners wired before wireMfdNav() so its initial activate() (which
  // may land on Page 1/2 if that's the saved default) is already observed.
  providerViews.forEach((view) => {
    wireHistoryPage(view);
    wireLimitsPage(view);
  });
  wireSettingsPage({ onDisplayModeChange: changeDisplayMode });
  wireMfdNav();
  providerViews.forEach((view) => renderFooterMetric(view));
  if (claudeView.root().hidden === false) {
    setGear(["opus"], claudeView); // positions Claude's default marker
  }
  providerViews.forEach((view) => startBurnAnimation(view)); // independent provider springs
}

window.addEventListener("DOMContentLoaded", init);
