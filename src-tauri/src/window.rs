//! Panel window: PIN state, hide-on-blur, native corner radius, drag-to-move
//! with a persisted override (D41), and the math that anchors the panel
//! right below the tray icon by default.

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::tray::TrayIcon;
use tauri::{AppHandle, Manager, PhysicalPosition, Rect, WebviewWindow, WindowEvent};

const PANEL_GAP: f64 = 4.0;

/// PIN button state (frontend): if active, hide-on-blur doesn't hide
/// the panel when it loses focus.
pub type PinnedState = Arc<Mutex<bool>>;

/// `#[tauri::command]` The frontend's PIN button pins/releases the panel.
#[tauri::command]
pub fn set_pinned(state: tauri::State<'_, PinnedState>, value: bool) {
    *lock(&state) = value;
}

// ─────────────────────────────────────────────────────────────────────────────
// D41: drag-to-move with a persisted override
// ─────────────────────────────────────────────────────────────────────────────

/// User-dragged position override (physical pixels), if any. `None` means
/// the default behavior applies: anchored under the tray icon on every open.
pub type PositionState = Arc<Mutex<Option<(f64, f64)>>>;

/// Timestamp of the last *programmatic* reposition (`position_under_tray` /
/// `position_at`). The `WindowEvent::Moved` handler in [`wire`] uses it to
/// tell those from a real user drag — same idiom as `tray.rs`'s
/// `REOPEN_GUARD`.
pub type AutoRepositionGuard = Arc<Mutex<Instant>>;

/// How long after a programmatic reposition a `Moved` event is still
/// considered an echo of it, not a user drag.
const AUTO_REPOSITION_GUARD: Duration = Duration::from_millis(250);
/// Minimum gap between disk writes while the user is actively dragging.
const POSITION_WRITE_THROTTLE: Duration = Duration::from_millis(150);

#[derive(Serialize, Deserialize)]
struct StoredPosition {
    x: f64,
    y: f64,
}

fn position_file(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("window-position.json"))
}

/// Reads the saved override, if any, at startup.
pub fn load_position(app: &AppHandle) -> Option<(f64, f64)> {
    let path = position_file(app)?;
    let data = fs::read_to_string(path).ok()?;
    let stored: StoredPosition = serde_json::from_str(&data).ok()?;
    Some((stored.x, stored.y))
}

fn save_position(app: &AppHandle, x: f64, y: f64) {
    let Some(path) = position_file(app) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&StoredPosition { x, y }) {
        let _ = fs::write(path, json);
    }
}

/// Clears the override (in memory and on disk) so the panel goes back to
/// anchoring under the tray icon. Wired to the tray menu's "Reset position"
/// and the Settings page button.
pub fn clear_position(app: &AppHandle, state: &PositionState) {
    *lock(state) = None;
    if let Some(path) = position_file(app) {
        let _ = fs::remove_file(path);
    }
}

/// Clears the override and, if the panel is currently visible, re-anchors it
/// under the tray icon right away — otherwise the change was invisible until
/// the next open/close cycle (D-review: the user shouldn't have to close and
/// reopen the panel just to see the reset take effect).
pub fn reset_position_now(
    app: &AppHandle,
    state: &PositionState,
    auto_guard: &AutoRepositionGuard,
) {
    clear_position(app, state);
    let Some(window) = app.get_webview_window("cluster") else {
        return;
    };
    if !window.is_visible().unwrap_or(false) {
        return;
    }
    let Some(tray) = app.try_state::<TrayIcon>() else {
        return;
    };
    if let Ok(Some(rect)) = tray.rect() {
        position_under_tray(&window, &rect, auto_guard);
    }
}

/// `#[tauri::command]` The Settings page's "Reset position" button.
#[tauri::command]
pub fn reset_position(
    app: AppHandle,
    position_state: tauri::State<'_, PositionState>,
    auto_guard: tauri::State<'_, AutoRepositionGuard>,
) {
    reset_position_now(&app, &position_state, &auto_guard);
}

/// Shows and focuses the panel if it isn't already visible — same
/// positioning logic as the tray's left-click show branch (`tray.rs`).
/// Called from `permission::mod.rs` (D42) when a NEW permission request
/// arrives: without this, a request that comes in while the panel is
/// hidden (the common case — hide-on-blur means it's hidden most of the
/// time unless PINned) would only ever show up as a blinking tray icon,
/// defeating the feature's whole point of approving without alt-tabbing.
/// A no-op if the panel is already visible (doesn't steal focus from an
/// unrelated, already-open panel) or if `PositionState`/`AutoRepositionGuard`
/// aren't managed yet (only reachable in the ~impossible case of a hook
/// connecting within milliseconds of app startup, before `.setup()` finishes).
pub fn show_for_permission(app: &AppHandle) {
    let Some(window) = app.get_webview_window("cluster") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        return;
    }
    let Some(position_state) = app.try_state::<PositionState>() else {
        return;
    };
    let Some(auto_guard) = app.try_state::<AutoRepositionGuard>() else {
        return;
    };

    let saved = *lock(&position_state);
    if let Some((x, y)) = saved {
        position_at(&window, x, y, &auto_guard);
    } else if let Some(tray) = app.try_state::<TrayIcon>() {
        if let Ok(Some(rect)) = tray.rect() {
            position_under_tray(&window, &rect, &auto_guard);
        }
    }
    let _ = show_panel(&window);
}

/// Shows the cluster as a native panel. `orderFrontRegardless` is essential
/// on macOS: a non-activating NSPanel can become key over another app's
/// fullscreen Space without trying (and failing) to activate this accessory
/// application first.
pub fn show_panel(window: &WebviewWindow) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        let window = window.clone();
        window.clone().run_on_main_thread(move || {
            let ns_window = window
                .ns_window()
                .expect("cc-autobahn: panel lost its native NSWindow");
            let panel: &objc2_app_kit::NSPanel = unsafe { &*ns_window.cast() };
            panel.orderFrontRegardless();
            panel.makeKeyWindow();
        })
    }

    #[cfg(not(target_os = "macos"))]
    {
        window.show()?;
        window.set_focus()
    }
}

/// Locks a mutex recovering from poison (a prior panic while held) instead of
/// propagating it — a background menu-bar app has no supervisor to restart it.
pub(crate) fn lock<T>(mutex: &std::sync::Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Applies the native rounded corners and wires hide-on-blur + drag-to-move
/// persistence (D41) onto `window`. Returns the last-blur-hide timestamp
/// guard, which `tray.rs` needs to debounce the tray-icon click that
/// immediately follows a hide-by-blur (D24), and the auto-reposition guard,
/// which `tray.rs` passes to `position_under_tray`/`position_at`.
pub fn wire(
    app: &tauri::App,
    window: &WebviewWindow,
    position_state: PositionState,
) -> tauri::Result<(Arc<Mutex<Option<Instant>>>, AutoRepositionGuard)> {
    // Native rounded corners (D24 addendum): with transparent:true,
    // Tauri/WebKit doesn't clip the CSS border-radius to the window's
    // alpha well (known bug, leaves a square "corner" on all 4 corners).
    // The NSWindow is clipped at the CALayer level, which antialiases correctly.
    #[cfg(target_os = "macos")]
    {
        configure_fullscreen_panel(window)?;

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
    let auto_reposition_guard: AutoRepositionGuard = Arc::new(Mutex::new(Instant::now()));

    let blur_flag = last_blur_hide.clone();
    let hide_window = window.clone();
    let pinned_for_blur = app.state::<PinnedState>().inner().clone();
    let auto_guard_for_move = auto_reposition_guard.clone();
    let position_for_move = position_state.clone();
    let app_handle = app.handle().clone();
    let last_disk_write: Arc<Mutex<Instant>> =
        Arc::new(Mutex::new(Instant::now() - POSITION_WRITE_THROTTLE));
    window.on_window_event(move |event| match event {
        WindowEvent::Focused(false) => {
            if *lock(&pinned_for_blur) {
                return; // PIN active (D24): don't hide on losing focus.
            }
            *lock(&blur_flag) = Some(Instant::now());
            // Flush the drag override so the final position survives even if
            // the last few Moved events were skipped by the write throttle.
            if let Some((x, y)) = *lock(&position_for_move) {
                save_position(&app_handle, x, y);
            }
            let _ = hide_window.hide();
        }
        WindowEvent::Moved(pos) => {
            // A reposition triggered by `position_under_tray`/`position_at`
            // also fires `Moved` — within the guard window it's an echo of
            // that, not a real drag, so it must not overwrite the override.
            if lock(&auto_guard_for_move).elapsed() < AUTO_REPOSITION_GUARD {
                return;
            }
            let (x, y) = (pos.x as f64, pos.y as f64);
            *lock(&position_for_move) = Some((x, y));

            let mut last_write = lock(&last_disk_write);
            if last_write.elapsed() >= POSITION_WRITE_THROTTLE {
                *last_write = Instant::now();
                drop(last_write);
                save_position(&app_handle, x, y);
            }
        }
        _ => {}
    });

    Ok((last_blur_hide, auto_reposition_guard))
}

/// Swizzles tao's `TaoWindow` into a real `NSPanel` subclass and applies the
/// AppKit contract required for an accessory window to join native fullscreen
/// Spaces. Changing only `NSWindow.collectionBehavior` is insufficient on the
/// affected macOS versions (D43); the runtime class is the material difference.
#[cfg(target_os = "macos")]
fn configure_fullscreen_panel(window: &WebviewWindow) -> tauri::Result<()> {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
    use objc2::{sel, ClassType};
    use objc2_app_kit::{
        NSPanel, NSScreenSaverWindowLevel, NSWindowCollectionBehavior, NSWindowStyleMask,
    };

    // A borderless NSPanel normally refuses key status. The override preserves
    // the existing clickable/keyboard-capable cluster behavior while the
    // NonactivatingPanel style keeps the fullscreen application frontmost.
    extern "C-unwind" fn can_become_key_window(_: &AnyObject, _: Sel) -> Bool {
        Bool::YES
    }

    fn panel_class() -> &'static AnyClass {
        static CLASS: OnceLock<&'static AnyClass> = OnceLock::new();
        CLASS.get_or_init(|| {
            let name = c"CcAutobahnPanel";
            if let Some(existing) = AnyClass::get(name) {
                return existing;
            }
            let mut builder = ClassBuilder::new(name, NSPanel::class())
                .expect("cc-autobahn: failed to create NSPanel subclass");
            unsafe {
                builder.add_method(
                    sel!(canBecomeKeyWindow),
                    can_become_key_window as extern "C-unwind" fn(_, _) -> _,
                );
            }
            builder.register()
        })
    }

    unsafe extern "C" {
        fn object_setClass(object: *mut c_void, class: *const AnyClass) -> *const AnyClass;
    }

    let raw = window.ns_window()?;
    let object: &AnyObject = unsafe { &*raw.cast() };
    let target = panel_class();
    let current = object.class();

    if current != target {
        // TaoWindow carries one small `focusable` ivar, so its allocation is
        // at least as large as our ivar-free NSPanel subclass. Refuse the swap
        // if a future tao/AppKit change invalidates that safety condition.
        assert!(
            current.instance_size() >= target.instance_size(),
            "cannot convert {} ({} bytes) to {} ({} bytes)",
            current.name().to_string_lossy(),
            current.instance_size(),
            target.name().to_string_lossy(),
            target.instance_size()
        );
        let previous = unsafe { object_setClass(raw, target) };
        assert_eq!(
            previous, current as *const AnyClass,
            "NSPanel conversion raced with another Objective-C class change"
        );
    }

    let panel: &NSPanel = unsafe { &*raw.cast() };
    panel.setStyleMask(NSWindowStyleMask::NonactivatingPanel);
    panel.setFloatingPanel(true);
    panel.setHidesOnDeactivate(false);
    panel.setBecomesKeyOnlyIfNeeded(false);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    // The screen-saver level is the lowest level consistently reported above
    // native fullscreen content on current macOS; unlike the discarded
    // maximumWindow experiment it does not outrank the highest system UI.
    panel.setLevel(NSScreenSaverWindowLevel);

    Ok(())
}

/// Anchors the panel right below the tray icon, centered horizontally and
/// clamped to the monitor that contains the icon so it doesn't go off-screen.
/// Default behavior (D24), used when no drag override is saved (D41).
pub fn position_under_tray(
    window: &WebviewWindow,
    tray_rect: &Rect,
    auto_guard: &AutoRepositionGuard,
) {
    let Ok(win_size) = window.outer_size() else {
        return;
    };
    let monitors = window.available_monitors().unwrap_or_default();

    // Finds the monitor that contains the icon's center, converting the
    // rect with a given scale (closure so it can be retried with the
    // correct scale below — D-review: the WINDOW's scale isn't reliable if
    // the tray lives on a monitor with a different DPI in multi-monitor setups).
    // Both axes must match: with displays stacked vertically the x-ranges
    // overlap, so an x-only check can pick the wrong monitor.
    let find_host = |scale: f64| {
        let pos = tray_rect.position.to_physical::<f64>(scale);
        let size = tray_rect.size.to_physical::<f64>(scale);
        let center_x = pos.x + size.width / 2.0;
        let center_y = pos.y + size.height / 2.0;
        monitors
            .iter()
            .find(|m| {
                let mp = m.position();
                let ms = m.size();
                (mp.x as f64) <= center_x
                    && center_x <= mp.x as f64 + ms.width as f64
                    && (mp.y as f64) <= center_y
                    && center_y <= mp.y as f64 + ms.height as f64
            })
            .cloned()
    };

    // First pass: window's scale, only to LOCATE the monitor.
    let guess_scale = window.scale_factor().unwrap_or(1.0);
    // Second pass: if the monitor has its own scale, that one is used for
    // the final calculation (matches the window's in the common case of
    // a single monitor or uniform DPI).
    let host = find_host(guess_scale);
    let scale = host
        .as_ref()
        .map(|m| m.scale_factor())
        .unwrap_or(guess_scale);

    let tray_pos = tray_rect.position.to_physical::<f64>(scale);
    let tray_size = tray_rect.size.to_physical::<f64>(scale);
    let tray_center_x = tray_pos.x + tray_size.width / 2.0;
    let mut x = tray_center_x - (win_size.width as f64) / 2.0;
    let y = tray_pos.y + tray_size.height + PANEL_GAP;

    if let Some(m) = host {
        let mp = m.position();
        let ms = m.size();
        let min_x = mp.x as f64 + PANEL_GAP;
        let max_x = mp.x as f64 + ms.width as f64 - win_size.width as f64 - PANEL_GAP;
        x = x.clamp(min_x, max_x.max(min_x));
    }

    *lock(auto_guard) = Instant::now();
    set_top_left(window, x, y, scale);
}

/// Places the panel at a saved drag override (D41), clamped to whichever
/// monitor currently contains that point — the saved spot may no longer be
/// on screen if a monitor was disconnected since it was dragged there.
pub fn position_at(window: &WebviewWindow, x: f64, y: f64, auto_guard: &AutoRepositionGuard) {
    let Ok(win_size) = window.outer_size() else {
        return;
    };
    let monitors = window.available_monitors().unwrap_or_default();

    let host = monitors.iter().find(|m| {
        let mp = m.position();
        let ms = m.size();
        (mp.x as f64) <= x
            && x <= mp.x as f64 + ms.width as f64
            && (mp.y as f64) <= y
            && y <= mp.y as f64 + ms.height as f64
    });
    // Same reasoning as `position_under_tray`: the host monitor's own scale,
    // not the window's current one — they can differ in multi-monitor setups
    // with mixed DPI, and the window may currently be on a different screen
    // than the saved point.
    let scale = host
        .map(|m| m.scale_factor())
        .unwrap_or_else(|| window.scale_factor().unwrap_or(1.0));

    let (mut clamped_x, mut clamped_y) = (x, y);
    if let Some(m) = host {
        let mp = m.position();
        let ms = m.size();
        let min_x = mp.x as f64 + PANEL_GAP;
        let max_x = mp.x as f64 + ms.width as f64 - win_size.width as f64 - PANEL_GAP;
        let min_y = mp.y as f64 + PANEL_GAP;
        let max_y = mp.y as f64 + ms.height as f64 - win_size.height as f64 - PANEL_GAP;
        clamped_x = clamped_x.clamp(min_x, max_x.max(min_x));
        clamped_y = clamped_y.clamp(min_y, max_y.max(min_y));
    }

    *lock(auto_guard) = Instant::now();
    set_top_left(window, clamped_x, clamped_y, scale);
}

/// Places the window's top-left corner at physical (x, y).
///
/// On macOS this is done natively instead of via `window.set_position`:
/// tao's `set_outer_position` flips the Y axis with
/// `CGDisplay::main().pixels_high()` — the display with KEYBOARD focus, in
/// physical pixels, mixed with logical points. With a second display stacked
/// above the built-in one, the panel landed a full screen-height off
/// (observed: window server placed it at Y=-1048, invisible, while tao
/// believed it was at y=70). `NSScreen` frames share one consistent
/// bottom-left global coordinate space, so flipping against the primary
/// screen (`screens[0]`, the one whose top-left is (0,0) in the global
/// top-left space) is exact for any display arrangement.
#[cfg(target_os = "macos")]
fn set_top_left(window: &WebviewWindow, x: f64, y: f64, scale: f64) {
    use objc2_app_kit::NSScreen;
    use objc2_foundation::NSPoint;

    let native = (|| {
        let mtm = objc2::MainThreadMarker::new()?;
        let primary = NSScreen::screens(mtm).firstObject()?;
        let ns_window = window.ns_window().ok()?;
        let ns_window: &objc2_app_kit::NSWindow = unsafe { &*ns_window.cast() };
        // setFrameTopLeftPoint takes the window's TOP-LEFT in bottom-left
        // global coordinates: flip Y against the primary screen's height.
        let primary_h = primary.frame().size.height;
        let point = NSPoint::new(x / scale, primary_h - y / scale);
        ns_window.setFrameTopLeftPoint(point);
        Some(())
    })();

    if native.is_none() {
        let _ = window.set_position(PhysicalPosition::new(x as i32, y as i32));
    }
}

/// Non-macOS platforms go through tao's positioning, which is correct there.
#[cfg(not(target_os = "macos"))]
fn set_top_left(window: &WebviewWindow, x: f64, y: f64, _scale: f64) {
    let _ = window.set_position(PhysicalPosition::new(x as i32, y as i32));
}
