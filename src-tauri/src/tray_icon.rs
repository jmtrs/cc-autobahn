// Tray icon as a progress ring (D-review: "the white dot doesn't serve any
// purpose, make it like the autonomy ring").
// Redraws the PNG pixel-by-pixel each time new autonomy data arrives
// (engine::poll or sensor::tail) — same "remaining" criterion as the panel's
// segment gauge: 100% = 5h window full, 0% = exhausted. No drawing deps:
// same hand-rolled pattern as scripts/make-tray-icon.mjs, but at
// runtime and with a hole (ring, not disc) so the arc can be painted.
use std::collections::BTreeMap;
use std::f64::consts::TAU;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

use tauri::tray::TrayIcon;
use tauri::{AppHandle, Manager, Wry};

use crate::providers::{ProviderId, RateLimitSnapshot, SourceQuality};

const S: u32 = 44; // 22pt @2x retina, same as the previous static icon
                   // D53: bumped outward (0.42/0.28 -> 0.46/0.32) rather than growing S itself —
                   // macOS auto-fits NSStatusItem's button image to the fixed menu-bar row
                   // height regardless of the source buffer's pixel size, so a bigger S risks
                   // being silently rescaled back down. Filling more of the same S×S canvas
                   // (ring keeps its ~0.14*S thickness, just sits closer to the edge) reliably
                   // reads as bigger without fighting that. Still ~1.7px of edge margin left.
const OUTER_R: f64 = S as f64 * 0.46;
const INNER_R: f64 = S as f64 * 0.32;
const TRACK_ALPHA: u8 = 55; // faint track, always visible (100% reference)
const ARC_ALPHA: u8 = 255; // progress arc, opaque
const ALERT_BLINK_MS: u64 = 450; // on/off half-period while critical (D37/D-review)
const EXCLAMATION_STEM_HALF_W: f64 = S as f64 * 0.05;
const EXCLAMATION_STEM_TOP_FRAC: f64 = 0.68; // * INNER_R above center
const EXCLAMATION_STEM_BOTTOM_FRAC: f64 = 0.05; // * INNER_R below center
const EXCLAMATION_DOT_R: f64 = S as f64 * 0.055;
const EXCLAMATION_DOT_CENTER_FRAC: f64 = 0.6; // * INNER_R below center

/// W203 VFD amber for the Linux tray. macOS uses alpha-only template
/// masking (no RGB — AppKit supplies the tint), so these are Linux-only.
/// AppIndicator has no template concept, so a 0-RGB pixel would be solid
/// black; the ring is painted this fixed amber instead — same color as
/// the rest of the VFD skin and `scripts/make-tray-icon-linux.mjs` (D55).
#[cfg(not(target_os = "macos"))]
const AMBER_R: u8 = 0xff;
#[cfg(not(target_os = "macos"))]
const AMBER_G: u8 = 0xb0;
#[cfg(not(target_os = "macos"))]
const AMBER_B: u8 = 0x00;

/// 5h billing window in minutes — same as WINDOW_MIN in main.js.
pub const WINDOW_MIN: f64 = 300.0;

/// Source precedence is provider-local. An official Claude reading blocks only
/// Claude's estimate; it never blocks a Codex reading or wins by arrival order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProgressSource {
    Estimated,
    Official,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ProgressCandidate {
    pct_remaining: f64,
    source: ProgressSource,
}

#[derive(Debug, Default)]
struct ProgressCandidates(BTreeMap<ProviderId, ProgressCandidate>);

impl ProgressCandidates {
    fn update(&mut self, provider: ProviderId, pct_remaining: f64, source: ProgressSource) -> bool {
        if !pct_remaining.is_finite() {
            return false;
        }
        if source == ProgressSource::Estimated
            && self
                .0
                .get(&provider)
                .is_some_and(|candidate| candidate.source == ProgressSource::Official)
        {
            return false;
        }
        self.0.insert(
            provider,
            ProgressCandidate {
                pct_remaining: pct_remaining.clamp(0.0, 100.0),
                source,
            },
        );
        true
    }

    fn remove(&mut self, provider: ProviderId) -> bool {
        self.0.remove(&provider).is_some()
    }

    fn summary(&self) -> Option<(ProviderId, f64)> {
        self.0
            .iter()
            .map(|(provider, candidate)| (*provider, candidate.pct_remaining))
            .min_by(|left, right| {
                left.1
                    .partial_cmp(&right.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.0.cmp(&right.0))
            })
    }

    fn summary_pct(&self) -> f64 {
        self.summary().map(|(_, pct)| pct).unwrap_or(100.0)
    }

    fn tooltip(&self) -> String {
        let Some((winner, _)) = self.summary() else {
            return "cc-autobahn · quota unavailable".into();
        };
        let mut parts: Vec<_> = self
            .0
            .iter()
            .map(|(provider, candidate)| {
                format!(
                    "{} {:.0}%",
                    provider_name(*provider),
                    candidate.pct_remaining
                )
            })
            .collect();
        parts.push(format!("showing {}", provider_name(winner)));
        format!("cc-autobahn · {}", parts.join(" · "))
    }
}

fn provider_name(provider: ProviderId) -> &'static str {
    match provider {
        ProviderId::Claude => "Claude",
        ProviderId::Codex => "Codex",
    }
}

/// Shared tray state. Candidates stay current even while an alert owns the
/// icon. The summary is the most urgent valid provider quota (minimum
/// remaining), never an average. A `Mutex` is required because provider
/// workers and the blink thread repaint concurrently.
struct TrayState {
    candidates: ProgressCandidates,
    alert: bool,
    pending_permission: bool,
}

fn state() -> &'static Mutex<TrayState> {
    static STATE: OnceLock<Mutex<TrayState>> = OnceLock::new();
    STATE.get_or_init(|| {
        Mutex::new(TrayState {
            candidates: ProgressCandidates::default(),
            alert: false,
            pending_permission: false,
        })
    })
}

static BLINK_THREAD_STARTED: AtomicBool = AtomicBool::new(false);

/// Records one provider's progress and repaints the conservative summary.
/// Official-over-estimated arbitration is isolated to that provider.
pub fn set_progress(
    app: &AppHandle,
    provider: ProviderId,
    pct_remaining: f64,
    source: ProgressSource,
) {
    let mut s = state().lock().unwrap();
    if !s.candidates.update(provider, pct_remaining, source) {
        return;
    }
    if !s.alert && !s.pending_permission {
        paint_summary(app, &s.candidates);
    } else {
        update_tooltip(app, &s.candidates);
    }
}

/// Removes a provider whose quota is explicitly unavailable. A stale snapshot
/// deliberately keeps its last-known value; App Server promotes it to
/// unavailable after its longer expiry window.
pub fn clear_progress(app: &AppHandle, provider: ProviderId) {
    let mut s = state().lock().unwrap();
    if s.candidates.remove(provider) {
        if !s.alert && !s.pending_permission {
            paint_summary(app, &s.candidates);
        } else {
            update_tooltip(app, &s.candidates);
        }
    }
}

/// Applies the normalized provider-neutral rate-limit contract to the tray.
pub fn sync_rate_limit_snapshot(app: &AppHandle, snapshot: &RateLimitSnapshot) {
    match snapshot.source_quality {
        SourceQuality::Official | SourceQuality::Estimated => {
            let source = if snapshot.source_quality == SourceQuality::Official {
                ProgressSource::Official
            } else {
                ProgressSource::Estimated
            };
            if let Some(primary) = &snapshot.primary {
                set_progress(app, snapshot.provider, 100.0 - primary.used_percent, source);
            } else {
                clear_progress(app, snapshot.provider);
            }
        }
        SourceQuality::Local | SourceQuality::Stale => {}
        SourceQuality::Unavailable => clear_progress(app, snapshot.provider),
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
    let changed = {
        let mut s = state().lock().unwrap();
        let changed = s.alert != active;
        s.alert = active;
        if changed && !active && !s.pending_permission {
            paint_summary(app, &s.candidates);
        }
        changed
    };
    if !changed {
        return;
    }
    if active {
        ensure_blink_thread(app.clone());
    }
}

/// Mirrors [`set_alert`]'s idempotent shape for a pending `PermissionRequest`
/// (D42) — kept as its own field rather than reusing `alert`, since the two
/// urgencies mean different things to a user glancing at the menu bar
/// (PACE/AUTO budget pressure vs "a session is blocked waiting on you"),
/// even though both share the same blink treatment. Called directly from
/// `permission::mod.rs`, not via IPC — nothing in the frontend drives this
/// (unlike `set_tray_alert`, which mirrors `redline.js`'s own threshold
/// logic), the backend already knows the queue state authoritatively.
pub fn set_permission_pending(app: &AppHandle, active: bool) {
    let changed = {
        let mut s = state().lock().unwrap();
        let changed = s.pending_permission != active;
        s.pending_permission = active;
        if changed && !active && !s.alert {
            paint_summary(app, &s.candidates);
        }
        changed
    };
    if !changed {
        return;
    }
    if active {
        ensure_blink_thread(app.clone());
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
        if !paint_blink_frame(&app, ARC_ALPHA) {
            thread::sleep(Duration::from_millis(200));
            continue;
        }
        thread::sleep(Duration::from_millis(ALERT_BLINK_MS));
        if !paint_blink_frame(&app, TRACK_ALPHA) {
            continue;
        }
        thread::sleep(Duration::from_millis(ALERT_BLINK_MS));
    });
}

/// Both urgencies keep the same blink thread running, but `pending_permission`
/// paints an exclamation mark inside the ring (D53) — a session is blocked
/// waiting on a human, which is a different kind of urgent than PACE/AUTO
/// budget pressure (`alert` alone). Template icons (D30) are alpha-only, so a
/// color difference was never on the table (D37); shape is the only channel
/// left to tell them apart at a glance, before the gate panel is even open.
fn paint_blink_frame(app: &AppHandle, alpha: u8) -> bool {
    let s = state().lock().unwrap();
    let active = s.alert || s.pending_permission;
    if active {
        let buf = if s.pending_permission {
            render_uniform_with_exclamation(alpha)
        } else {
            render_uniform(alpha)
        };
        paint(app, buf);
    }
    active
}

fn paint(app: &AppHandle, buf: Vec<u8>) {
    let Some(tray) = app.try_state::<TrayIcon<Wry>>() else {
        return;
    };
    let image = tauri::image::Image::new_owned(buf, S, S);
    paint_tray_icon(&tray, image);
}

// Platform fork kept in one place: macOS preserves the template flag on
// every redraw (set_icon() alone repaints fixed black without adapting to
// light/dark — real bug, D30); Linux/AppIndicator has no template concept,
// and set_icon_with_as_template has been reported to drop the icon on some
// libayatana versions, so Linux uses the plain setter (D55).
#[cfg(target_os = "macos")]
fn paint_tray_icon(tray: &TrayIcon<Wry>, image: tauri::image::Image) {
    let _ = tray.set_icon_with_as_template(Some(image), true);
}

#[cfg(not(target_os = "macos"))]
fn paint_tray_icon(tray: &TrayIcon<Wry>, image: tauri::image::Image) {
    let _ = tray.set_icon(Some(image));
}

fn paint_summary(app: &AppHandle, candidates: &ProgressCandidates) {
    let pct = candidates.summary_pct();
    paint(app, render(pct));
    update_tooltip(app, candidates);
}

fn update_tooltip(app: &AppHandle, candidates: &ProgressCandidates) {
    let Some(tray) = app.try_state::<TrayIcon<Wry>>() else {
        return;
    };
    let tooltip = candidates.tooltip();
    let _ = tray.set_tooltip(Some(tooltip));
}

/// Raw RGBA of the ring at `pct` swept: opaque black on the arc, faint black
/// on the rest of the track. Thin wrapper over `render_ring` — see there for
/// the shared geometry.
fn render(pct: f64) -> Vec<u8> {
    let sweep = pct / 100.0 * TAU;
    render_ring(|angle| {
        if angle <= sweep {
            ARC_ALPHA
        } else {
            TRACK_ALPHA
        }
    })
}

/// Whole ring at one flat alpha — the alert blink's two frames.
fn render_uniform(alpha: u8) -> Vec<u8> {
    render_ring(|_angle| alpha)
}

/// Whole ring plus an exclamation mark in its hole, both at the same alpha —
/// the pending-permission blink's two frames (D53).
fn render_uniform_with_exclamation(alpha: u8) -> Vec<u8> {
    let mut buf = render_uniform(alpha);
    draw_exclamation(&mut buf, alpha);
    buf
}

/// Paints a filled exclamation mark (stem + dot) into the ring's empty
/// center hole. Geometry is fractions of `INNER_R` so it stays clear of the
/// ring annulus by construction — verified in `exclamation_stays_inside_hole`.
fn draw_exclamation(buf: &mut [u8], alpha: u8) {
    let cx = S as f64 / 2.0;
    let cy = S as f64 / 2.0;
    let stem_top = cy - INNER_R * EXCLAMATION_STEM_TOP_FRAC;
    let stem_bottom = cy + INNER_R * EXCLAMATION_STEM_BOTTOM_FRAC;
    let dot_cy = cy + INNER_R * EXCLAMATION_DOT_CENTER_FRAC;
    for y in 0..S {
        for x in 0..S {
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5;
            let in_stem =
                (px - cx).abs() <= EXCLAMATION_STEM_HALF_W && py >= stem_top && py <= stem_bottom;
            let dx = px - cx;
            let dy = py - dot_cy;
            let in_dot = (dx * dx + dy * dy).sqrt() <= EXCLAMATION_DOT_R;
            if in_stem || in_dot {
                let i = ((y * S + x) * 4) as usize;
                write_pixel(buf, i, alpha);
            }
        }
    }
}

/// Writes one RGBA pixel into `buf` at byte offset `i`. macOS paints alpha
/// only (the icon is a template mask; AppKit supplies the tint, so RGB stays
/// 0). Linux paints the amber VFD color — AppIndicator has no template
/// concept, so a 0-RGB pixel would render solid black (D55). Geometry stays
/// shared; only this write forks.
#[cfg(target_os = "macos")]
#[inline]
fn write_pixel(buf: &mut [u8], i: usize, alpha: u8) {
    buf[i + 3] = alpha;
}

#[cfg(not(target_os = "macos"))]
#[inline]
fn write_pixel(buf: &mut [u8], i: usize, alpha: u8) {
    buf[i] = AMBER_R;
    buf[i + 1] = AMBER_G;
    buf[i + 2] = AMBER_B;
    buf[i + 3] = alpha;
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
            write_pixel(&mut buf, i, alpha_at(angle));
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_uses_the_most_urgent_provider_without_averaging() {
        let mut candidates = ProgressCandidates::default();
        assert!(candidates.update(ProviderId::Claude, 72.0, ProgressSource::Official));
        assert!(candidates.update(ProviderId::Codex, 38.0, ProgressSource::Official));
        assert_eq!(candidates.summary(), Some((ProviderId::Codex, 38.0)));
    }

    #[test]
    fn official_precedence_is_isolated_per_provider() {
        let mut candidates = ProgressCandidates::default();
        assert!(candidates.update(ProviderId::Claude, 70.0, ProgressSource::Official));
        assert!(!candidates.update(ProviderId::Claude, 20.0, ProgressSource::Estimated));
        assert!(candidates.update(ProviderId::Codex, 45.0, ProgressSource::Estimated));
        assert_eq!(candidates.summary(), Some((ProviderId::Codex, 45.0)));
    }

    #[test]
    fn removing_an_unavailable_provider_reveals_the_other_candidate() {
        let mut candidates = ProgressCandidates::default();
        candidates.update(ProviderId::Claude, 65.0, ProgressSource::Official);
        candidates.update(ProviderId::Codex, 10.0, ProgressSource::Official);
        assert!(candidates.remove(ProviderId::Codex));
        assert_eq!(candidates.summary(), Some((ProviderId::Claude, 65.0)));
        assert!(candidates.remove(ProviderId::Claude));
        assert_eq!(candidates.summary(), None);
    }

    #[test]
    fn invalid_values_do_not_poison_the_summary() {
        let mut candidates = ProgressCandidates::default();
        assert!(!candidates.update(ProviderId::Codex, f64::NAN, ProgressSource::Official));
        assert_eq!(candidates.summary(), None);
        assert!(candidates.update(ProviderId::Codex, 140.0, ProgressSource::Official));
        assert_eq!(candidates.summary(), Some((ProviderId::Codex, 100.0)));
    }

    #[test]
    fn tooltip_reports_all_candidates_and_deterministic_winner() {
        let mut candidates = ProgressCandidates::default();
        assert_eq!(candidates.tooltip(), "cc-autobahn · quota unavailable");
        candidates.update(ProviderId::Codex, 40.0, ProgressSource::Official);
        candidates.update(ProviderId::Claude, 40.0, ProgressSource::Official);
        assert_eq!(
            candidates.tooltip(),
            "cc-autobahn · Claude 40% · Codex 40% · showing Claude"
        );
    }

    fn pixel_alpha(buf: &[u8], x: u32, y: u32) -> u8 {
        buf[((y * S + x) * 4 + 3) as usize]
    }

    fn distance_from_center(x: u32, y: u32) -> f64 {
        let cx = S as f64 / 2.0;
        let cy = S as f64 / 2.0;
        let dx = x as f64 + 0.5 - cx;
        let dy = y as f64 + 0.5 - cy;
        (dx * dx + dy * dy).sqrt()
    }

    #[test]
    fn plain_alert_blink_leaves_the_hole_empty() {
        let buf = render_uniform(ARC_ALPHA);
        for y in 0..S {
            for x in 0..S {
                if distance_from_center(x, y) < INNER_R {
                    assert_eq!(
                        pixel_alpha(&buf, x, y),
                        0,
                        "plain alert frame must not paint inside the ring's hole"
                    );
                }
            }
        }
    }

    #[test]
    fn exclamation_stays_inside_hole_and_is_opaque() {
        // draw_exclamation in isolation, on a blank buffer — render_uniform's
        // ring annulus itself legitimately spans INNER_R..=OUTER_R, so
        // asserting on the combined frame would conflate ring pixels with
        // glyph pixels.
        let mut buf = vec![0u8; (S * S * 4) as usize];
        draw_exclamation(&mut buf, ARC_ALPHA);
        let mut painted_inside_hole = false;
        for y in 0..S {
            for x in 0..S {
                if pixel_alpha(&buf, x, y) == 0 {
                    continue;
                }
                assert!(
                    distance_from_center(x, y) < INNER_R,
                    "exclamation pixel at ({x},{y}) leaked outside the ring's hole"
                );
                painted_inside_hole = true;
            }
        }
        assert!(
            painted_inside_hole,
            "exclamation mark did not paint anything inside the hole"
        );
    }

    #[test]
    fn exclamation_frame_differs_from_plain_frame() {
        assert_ne!(
            render_uniform(ARC_ALPHA),
            render_uniform_with_exclamation(ARC_ALPHA)
        );
    }

    fn pixel_rgb(buf: &[u8], x: u32, y: u32) -> (u8, u8, u8) {
        let i = ((y * S + x) * 4) as usize;
        (buf[i], buf[i + 1], buf[i + 2])
    }

    // The D55 fork locks both sides of the contract: macOS keeps RGB at 0 so
    // AppKit's template tint adapts to light/dark; Linux paints amber so the
    // ring is visible under AppIndicator (no template concept). A future
    // refactor must not silently break either.

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_ring_keeps_rgb_zero_for_template_tinting() {
        let buf = render(50.0);
        let mut any_ring_pixel = false;
        for y in 0..S {
            for x in 0..S {
                if (INNER_R..=OUTER_R).contains(&distance_from_center(x, y)) {
                    any_ring_pixel = true;
                    assert_eq!(
                        pixel_rgb(&buf, x, y),
                        (0, 0, 0),
                        "macOS template ring must keep RGB 0 at ({x},{y})"
                    );
                }
            }
        }
        assert!(any_ring_pixel);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_exclamation_keeps_rgb_zero() {
        let mut buf = vec![0u8; (S * S * 4) as usize];
        draw_exclamation(&mut buf, ARC_ALPHA);
        for y in 0..S {
            for x in 0..S {
                if pixel_alpha(&buf, x, y) > 0 {
                    assert_eq!(
                        pixel_rgb(&buf, x, y),
                        (0, 0, 0),
                        "macOS exclamation must keep RGB 0 at ({x},{y})"
                    );
                }
            }
        }
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_ring_paints_amber_vfd_rgb() {
        let buf = render(50.0);
        let mut any_ring_pixel = false;
        for y in 0..S {
            for x in 0..S {
                if (INNER_R..=OUTER_R).contains(&distance_from_center(x, y)) {
                    any_ring_pixel = true;
                    assert_eq!(
                        pixel_rgb(&buf, x, y),
                        (AMBER_R, AMBER_G, AMBER_B),
                        "Linux ring must be amber VFD at ({x},{y})"
                    );
                }
            }
        }
        assert!(any_ring_pixel);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_exclamation_is_amber_vfd_rgb() {
        let buf = render_uniform_with_exclamation(ARC_ALPHA);
        let mut painted_amber = false;
        for y in 0..S {
            for x in 0..S {
                // Glyph lives in the ring's hole; the annulus is already
                // covered by linux_ring_paints_amber_vfd_rgb.
                if pixel_alpha(&buf, x, y) > 0 && distance_from_center(x, y) < INNER_R {
                    assert_eq!(
                        pixel_rgb(&buf, x, y),
                        (AMBER_R, AMBER_G, AMBER_B),
                        "Linux exclamation must be amber at ({x},{y})"
                    );
                    painted_amber = true;
                }
            }
        }
        assert!(painted_amber, "exclamation did not paint amber in the hole");
    }
}
