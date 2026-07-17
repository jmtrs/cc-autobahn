// Redline: when PACE or AUTO cross a critical threshold, the whole
// instrument reacts (screen tint, segment bar, footer value, speedometer),
// not just the footer text — same "physical gauge under stress" language
// as the PRND pulse (setGear() in trip-computer.js) and the 7d .screen.warn
// tint, reused here instead of inventing a new mechanism.

const PACE_CRITICAL_PCT = 50; // recent pace >= 50% above the block average
const AUTO_CRITICAL_MIN = 15; // <= 15 min of autonomy left

let wasRedline = false;
let trayInvoke = null;

/** One-time Tauri IPC wiring for the tray icon alert bridge (set_tray_alert,
 *  tray_icon.rs) — same guarded-import pattern as pin-button.js. Skipped
 *  entirely under plain `vite` (no Tauri, no IPC). Called once from
 *  main.js's init(), fire-and-forget like the other wire*() calls there. */
export async function wireRedlineTray() {
  if (!("__TAURI_INTERNALS__" in window)) return;
  const { invoke } = await import("@tauri-apps/api/core");
  trayInvoke = invoke;
}

/** Fire-and-forget: the tray icon just mirrors this screen's `critical`
 *  state, no independent threshold logic in Rust to keep in sync. */
function notifyTrayAlert(active) {
  if (!trayInvoke) return;
  trayInvoke("set_tray_alert", { active }).catch((e) =>
    console.error("[redline] set_tray_alert:", e)
  );
}

/** Re-triggerable one-shot animation, same pattern as setGear()'s .pulse
 *  (trip-computer.js): force a reflow so it can replay even if the class
 *  never fully left the element, clean up via animationend. */
function pulseOnce(el, className) {
  if (!el) return;
  el.classList.remove(className);
  void el.offsetWidth;
  el.classList.add(className);
  el.addEventListener("animationend", () => el.classList.remove(className), { once: true });
}

/** Evaluates PACE/AUTO on every footer render (both, regardless of which one
 *  is currently displayed) and drives the redline state: a sustained tint
 *  while critical, plus a one-shot flash/spike the instant it's crossed. */
export function updateRedline(pacePct, autoMinutesLeft) {
  const critical =
    (pacePct != null && pacePct >= PACE_CRITICAL_PCT) ||
    (autoMinutesLeft != null && autoMinutesLeft <= AUTO_CRITICAL_MIN);

  const screen = document.querySelector(".screen");
  const segments = document.getElementById("segments");
  if (!screen || !segments) return;

  screen.classList.toggle("redline", critical);
  segments.classList.toggle("redline", critical);

  // Only notify the tray on the edge — the IPC round-trip has real cost,
  // unlike the local classList.toggle() above, so it shouldn't fire on
  // every render while sustained-critical holds steady.
  if (critical !== wasRedline) {
    notifyTrayAlert(critical);
  }

  if (critical && !wasRedline) {
    pulseOnce(screen, "redline-enter");
    pulseOnce(document.getElementById("footer-metric-value"), "spike");
    pulseOnce(document.getElementById("burn"), "spike");
    segments.querySelectorAll(".seg").forEach((seg, i) => {
      seg.style.setProperty("--i", i);
      pulseOnce(seg, "ripple");
    });
  }
  wasRedline = critical;
}
