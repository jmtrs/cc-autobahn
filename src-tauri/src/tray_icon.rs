// Tray icon as a progress ring (D-review: "the white dot doesn't serve any
// purpose, make it like the autonomy ring").
// Redraws the PNG pixel-by-pixel each time new autonomy data arrives
// (engine::poll or sensor::tail) — same "remaining" criterion as the panel's
// segment gauge: 100% = 5h window full, 0% = exhausted. No drawing deps:
// same hand-rolled pattern as scripts/make-tray-icon.mjs, but at
// runtime and with a hole (ring, not disc) so the arc can be painted.
use std::f64::consts::TAU;

use tauri::tray::TrayIcon;
use tauri::{AppHandle, Manager, Wry};

const S: u32 = 44; // 22pt @2x retina, same as the previous static icon
const OUTER_R: f64 = S as f64 * 0.42;
const INNER_R: f64 = S as f64 * 0.28;
const TRACK_ALPHA: u8 = 55; // faint track, always visible (100% reference)
const ARC_ALPHA: u8 = 255; // progress arc, opaque

/// 5h billing window in minutes — same as WINDOW_MIN in main.js.
pub const WINDOW_MIN: f64 = 300.0;

/// Redraws the tray icon with the ring at the given `pct_remaining`
/// (0–100, clamped). If the tray isn't managed yet (very early startup)
/// this does nothing — it will be retried on the next tick.
pub fn set_progress(app: &AppHandle, pct_remaining: f64) {
    let Some(tray) = app.try_state::<TrayIcon<Wry>>() else {
        return;
    };
    let pct = pct_remaining.clamp(0.0, 100.0);
    let image = tauri::image::Image::new_owned(render(pct), S, S);
    // set_icon() alone does NOT preserve macOS's "template" flag (the icon
    // gets repainted as a normal image, fixed black, without adapting to
    // light/dark mode — bug found during visual review). set_icon_with_as_template()
    // sets both atomically on every redraw.
    let _ = tray.set_icon_with_as_template(Some(image), true);
}

/// Raw RGBA of the ring: opaque black on the swept arc, faint black on
/// the rest of the track. In template mode (D24) macOS ignores RGB and uses
/// alpha as a mask — the track's low alpha reads as "dimmed".
fn render(pct: f64) -> Vec<u8> {
    let cx = S as f64 / 2.0;
    let cy = S as f64 / 2.0;
    let sweep = pct / 100.0 * TAU;
    let mut buf = vec![0u8; (S * S * 4) as usize];
    for y in 0..S {
        for x in 0..S {
            let dx = x as f64 + 0.5 - cx;
            let dy = y as f64 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if !(INNER_R..=OUTER_R).contains(&d) {
                continue; // outside the ring: transparent (buf is already 0)
            }
            // Angle from the top (12 o'clock), clockwise, in [0, TAU).
            let mut angle = dx.atan2(-dy);
            if angle < 0.0 {
                angle += TAU;
            }
            let on = angle <= sweep;
            let i = ((y * S + x) * 4) as usize;
            buf[i + 3] = if on { ARC_ALPHA } else { TRACK_ALPHA };
        }
    }
    buf
}
