//! Panel window: PIN state, hide-on-blur, native corner radius, and the
//! math that anchors the panel right below the tray icon.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use tauri::{Manager, PhysicalPosition, Rect, WebviewWindow, WindowEvent};

const PANEL_GAP: f64 = 4.0;

/// PIN button state (frontend): if active, hide-on-blur doesn't hide
/// the panel when it loses focus.
pub type PinnedState = Arc<Mutex<bool>>;

/// `#[tauri::command]` The frontend's PIN button pins/releases the panel.
#[tauri::command]
pub fn set_pinned(state: tauri::State<'_, PinnedState>, value: bool) {
    *lock(&state) = value;
}

/// Locks a mutex recovering from poison (a prior panic while held) instead of
/// propagating it — a background menu-bar app has no supervisor to restart it.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Applies the native rounded corners and wires hide-on-blur onto `window`.
/// Returns the last-blur-hide timestamp guard, which `tray.rs` needs to debounce
/// the tray-icon click that immediately follows a hide-by-blur (D24).
pub fn wire(
    app: &tauri::App,
    window: &WebviewWindow,
) -> tauri::Result<Arc<Mutex<Option<Instant>>>> {
    // Native rounded corners (D24 addendum): with transparent:true,
    // Tauri/WebKit doesn't clip the CSS border-radius to the window's
    // alpha well (known bug, leaves a square "corner" on all 4 corners).
    // The NSWindow is clipped at the CALayer level, which antialiases correctly.
    #[cfg(target_os = "macos")]
    {
        let ns_window: &objc2_app_kit::NSWindow = unsafe { &*window.ns_window()?.cast() };
        if let Some(content_view) = ns_window.contentView() {
            content_view.setWantsLayer(true);
            if let Some(layer) = content_view.layer() {
                layer.setCornerRadius(12.0);
                layer.setMasksToBounds(true);
            }
        }
    }

    let last_blur_hide: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));

    let blur_flag = last_blur_hide.clone();
    let hide_window = window.clone();
    let pinned_for_blur = app.state::<PinnedState>().inner().clone();
    window.on_window_event(move |event| {
        if let WindowEvent::Focused(false) = event {
            if *lock(&pinned_for_blur) {
                return; // PIN active (D24): don't hide on losing focus.
            }
            *lock(&blur_flag) = Some(Instant::now());
            let _ = hide_window.hide();
        }
    });

    Ok(last_blur_hide)
}

/// Anchors the panel right below the tray icon, centered horizontally and
/// clamped to the monitor that contains the icon so it doesn't go off-screen.
pub fn position_under_tray(window: &WebviewWindow, tray_rect: &Rect) {
    let Ok(win_size) = window.outer_size() else {
        return;
    };
    let monitors = window.available_monitors().unwrap_or_default();

    // Finds the monitor that contains the icon's center, converting the
    // rect with a given scale (closure so it can be retried with the
    // correct scale below — D-review: the WINDOW's scale isn't reliable if
    // the tray lives on a monitor with a different DPI in multi-monitor setups).
    let find_host = |scale: f64| {
        let pos = tray_rect.position.to_physical::<f64>(scale);
        let size = tray_rect.size.to_physical::<f64>(scale);
        let center_x = pos.x + size.width / 2.0;
        monitors
            .iter()
            .find(|m| {
                let mp = m.position();
                let ms = m.size();
                (mp.x as f64) <= center_x && center_x <= mp.x as f64 + ms.width as f64
            })
            .cloned()
    };

    // First pass: window's scale, only to LOCATE the monitor.
    let guess_scale = window.scale_factor().unwrap_or(1.0);
    // Second pass: if the monitor has its own scale, that one is used for
    // the final calculation (matches the window's in the common case of
    // a single monitor or uniform DPI).
    let scale = find_host(guess_scale)
        .map(|m| m.scale_factor())
        .unwrap_or(guess_scale);

    let tray_pos = tray_rect.position.to_physical::<f64>(scale);
    let tray_size = tray_rect.size.to_physical::<f64>(scale);
    let tray_center_x = tray_pos.x + tray_size.width / 2.0;
    let mut x = tray_center_x - (win_size.width as f64) / 2.0;
    let y = tray_pos.y + tray_size.height + PANEL_GAP;

    if let Some(m) = find_host(scale) {
        let mp = m.position();
        let ms = m.size();
        let min_x = mp.x as f64 + PANEL_GAP;
        let max_x = mp.x as f64 + ms.width as f64 - win_size.width as f64 - PANEL_GAP;
        x = x.clamp(min_x, max_x.max(min_x));
    }

    let _ = window.set_position(PhysicalPosition::new(x as i32, y as i32));
}
