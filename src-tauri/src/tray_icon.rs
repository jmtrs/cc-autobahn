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
const OUTER_R: f64 = S as f64 * 0.42;
const INNER_R: f64 = S as f64 * 0.28;
const TRACK_ALPHA: u8 = 55; // faint track, always visible (100% reference)
const ARC_ALPHA: u8 = 255; // progress arc, opaque
const ALERT_BLINK_MS: u64 = 450; // on/off half-period while critical (D37/D-review)

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

/// Either urgency (`alert` or `pending_permission`) keeps the blink thread
/// running — the ring doesn't need to visually distinguish which reason to a
/// user glancing at the menu bar, the gate panel is where that distinction
/// actually matters.
fn paint_blink_frame(app: &AppHandle, alpha: u8) -> bool {
    let s = state().lock().unwrap();
    let active = s.alert || s.pending_permission;
    if active {
        paint(app, render_uniform(alpha));
    }
    active
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
}
