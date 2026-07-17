// Tray icon as a progress ring (D-review: "the white dot doesn't serve any
// purpose, make it like the autonomy ring").
// Redraws the PNG pixel-by-pixel each time new autonomy data arrives
// (engine::poll or sensor::tail) — same "remaining" criterion as the panel's
// segment gauge: 100% = 5h window full, 0% = exhausted. No drawing deps:
// same hand-rolled pattern as scripts/make-tray-icon.mjs, but at
// runtime and with a hole (ring, not disc) so the arc can be painted.
use std::f64::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use tauri::tray::TrayIcon;
use tauri::{AppHandle, Manager, Wry};

const S: u32 = 44; // 22pt @2x retina, same as the previous static icon
const OUTER_R: f64 = S as f64 * 0.42;
const INNER_R: f64 = S as f64 * 0.28;
const TRACK_ALPHA: u8 = 55; // faint track, always visible (100% reference)
const ARC_ALPHA: u8 = 255; // progress arc, opaque
const ALERT_BLINK_MS: u64 = 450; // on/off half-period while critical (D37/D-review)

/// 5h billing window in minutes — same as WINDOW_MIN in main.js.
pub const WINDOW_MIN: f64 = 300.0;

/// Shared tray state: the real progress value (kept up to date even while an
/// alert is painting over it) plus whether the redline alert is active.
/// A `Mutex` here isn't optional — `set_progress` (called from `engine`'s
/// and `sensor`'s own dedicated threads) and the blink thread below both
/// repaint the same tray icon, and without one they'd race each other.
struct TrayState {
    pct_remaining: f64,
    alert: bool,
}

fn state() -> &'static Mutex<TrayState> {
    static STATE: OnceLock<Mutex<TrayState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(TrayState {
            pct_remaining: 100.0,
            alert: false,
        })
    })
}

static BLINK_THREAD_STARTED: AtomicBool = AtomicBool::new(false);

/// Redraws the tray icon with the ring at the given `pct_remaining`
/// (0–100, clamped). If the tray isn't managed yet (very early startup)
/// this does nothing — it will be retried on the next tick. While the
/// redline alert is active, the value is only remembered (not painted) —
/// the blink thread owns the icon until `set_alert(app, false)` restores it.
pub fn set_progress(app: &AppHandle, pct_remaining: f64) {
    let pct = pct_remaining.clamp(0.0, 100.0);
    let alert = {
        let mut s = state().lock().unwrap();
        s.pct_remaining = pct;
        s.alert
    };
    if !alert {
        paint(app, render(pct));
    }
}

/// `#[tauri::command]` Bridges the frontend's redline state (PACE/AUTO
/// critical, `redline.js`) to the tray — the frontend is the single source
/// of truth for "critical" (it already weighs both PACE and AUTO with all
/// their nuance), the tray just reflects it instead of re-deriving its own
/// partial version of the same threshold logic.
#[tauri::command]
pub fn set_tray_alert(app: AppHandle, active: bool) {
    set_alert(&app, active);
}

/// Idempotent: a no-op if `active` matches the current state, so callers
/// (redline.js) don't need to track their own edge-detection just to avoid
/// spamming this.
pub fn set_alert(app: &AppHandle, active: bool) {
    let (changed, pct) = {
        let mut s = state().lock().unwrap();
        let changed = s.alert != active;
        s.alert = active;
        (changed, s.pct_remaining)
    };
    if !changed {
        return;
    }
    if active {
        ensure_blink_thread(app.clone());
    } else {
        // Restore the accurate ring immediately instead of waiting for the
        // blink thread to notice on its next poll.
        paint(app, render(pct));
    }
}

/// Spawned exactly once for the process's lifetime (same "dedicated thread
/// per concern" shape as `engine`/`burn`/`sensor`, D16) — idles at a cheap
/// 200ms poll while inactive, blinks the ring between the two alpha levels
/// the normal render already uses (`ARC_ALPHA`/`TRACK_ALPHA`) while active.
/// Never fully blank: a solid on/off blink between "bright" and "dim" reads
/// as an alert without ever looking like the icon just disappeared.
fn ensure_blink_thread(app: AppHandle) {
    if BLINK_THREAD_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    thread::spawn(move || loop {
        if !state().lock().unwrap().alert {
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        paint(&app, render_uniform(ARC_ALPHA));
        thread::sleep(Duration::from_millis(ALERT_BLINK_MS));
        if !state().lock().unwrap().alert {
            continue;
        }
        paint(&app, render_uniform(TRACK_ALPHA));
        thread::sleep(Duration::from_millis(ALERT_BLINK_MS));
    });
}

fn paint(app: &AppHandle, buf: Vec<u8>) {
    let Some(tray) = app.try_state::<TrayIcon<Wry>>() else {
        return;
    };
    let image = tauri::image::Image::new_owned(buf, S, S);
    // set_icon() alone does NOT preserve macOS's "template" flag (the icon
    // gets repainted as a normal image, fixed black, without adapting to
    // light/dark mode — bug found during visual review). set_icon_with_as_template()
    // sets both atomically on every redraw.
    let _ = tray.set_icon_with_as_template(Some(image), true);
}

/// Raw RGBA of the ring at `pct` swept: opaque black on the arc, faint black
/// on the rest of the track. Thin wrapper over `render_ring` — see there for
/// the shared geometry.
fn render(pct: f64) -> Vec<u8> {
    let sweep = pct / 100.0 * TAU;
    render_ring(|angle| if angle <= sweep { ARC_ALPHA } else { TRACK_ALPHA })
}

/// Whole ring at one flat alpha — the alert blink's two frames.
fn render_uniform(alpha: u8) -> Vec<u8> {
    render_ring(|_angle| alpha)
}

/// Shared ring geometry: in template mode (D24) macOS ignores RGB and uses
/// alpha as a mask. `alpha_at(angle)` decides each ring pixel's alpha from
/// its angle from the top (12 o'clock), clockwise, in `[0, TAU)`.
fn render_ring(alpha_at: impl Fn(f64) -> u8) -> Vec<u8> {
    let cx = S as f64 / 2.0;
    let cy = S as f64 / 2.0;
    let mut buf = vec![0u8; (S * S * 4) as usize];
    for y in 0..S {
        for x in 0..S {
            let dx = x as f64 + 0.5 - cx;
            let dy = y as f64 + 0.5 - cy;
            let d = (dx * dx + dy * dy).sqrt();
            if !(INNER_R..=OUTER_R).contains(&d) {
                continue; // outside the ring: transparent (buf is already 0)
            }
            let mut angle = dx.atan2(-dy);
            if angle < 0.0 {
                angle += TAU;
            }
            let i = ((y * S + x) * 4) as usize;
            buf[i + 3] = alpha_at(angle);
        }
    }
    buf
}
