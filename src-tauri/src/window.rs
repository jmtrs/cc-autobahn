//! Panel window: PIN state, hide-on-blur, native corner radius, drag-to-move
//! with a persisted override (D41), and the math that anchors the panel
//! right below the tray icon by default.

use std::fs;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::tray::TrayIcon;
use tauri::{AppHandle, LogicalSize, Manager, PhysicalPosition, Rect, WebviewWindow, WindowEvent};

const PANEL_GAP: f64 = 4.0;

fn panel_y_below_tray(tray_y: f64, tray_height: f64, work_top: f64) -> f64 {
    (tray_y + tray_height + PANEL_GAP).max(work_top + PANEL_GAP)
}

/// PIN button state (frontend): if active, hide-on-blur doesn't hide
/// the panel when it loses focus.
pub type PinnedState = Arc<Mutex<bool>>;

/// Settings-page checkbox: whether a NEW permission request should auto-show
/// the hidden panel at all. Default `true` — matches the pre-existing D42
/// behavior; this is an opt-out, not a new default. A newtype, not a plain
/// `Arc<Mutex<bool>>` alias (same reasoning as `TrayAnchorState`): Tauri's
/// state map is keyed by concrete type, and `PinnedState` is already
/// `Arc<Mutex<bool>>` — two `.manage()` calls with the same underlying type
/// panic at runtime ("state ... is already being managed").
pub struct AutoShowOnPermissionState(pub Arc<Mutex<bool>>);

/// True only for the span between `show_for_permission` actually transitioning
/// the panel hidden→visible and that visibility being resolved one way or
/// another (queue empties, or the user grabs manual control via the tray
/// icon). Lets the resolve path (`maybe_close_after_permission`) tell "I
/// opened myself for this notification" apart from "the panel was already
/// open for some other reason" without a second source of truth for
/// visibility itself. Newtype for the same reason as `AutoShowOnPermissionState`.
pub struct AutoOpenedByPermissionState(pub Arc<Mutex<bool>>);

/// `#[tauri::command]` The Settings page's "auto-open on request" checkbox.
#[tauri::command]
pub fn set_auto_show_on_permission(
    state: tauri::State<'_, AutoShowOnPermissionState>,
    value: bool,
) {
    *lock(&state.0) = value;
}

/// Serializes display-mode resize and clamp as one native transition.
pub type DisplayModeTransition = Arc<Mutex<()>>;

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    Claude,
    Codex,
    Both,
}

const SINGLE_PROVIDER_SIZE: (f64, f64) = (550.0, 150.0);
const DUAL_PROVIDER_SIZE: (f64, f64) = (550.0, 290.0);

fn display_mode_size(mode: DisplayMode) -> (f64, f64) {
    match mode {
        DisplayMode::Claude | DisplayMode::Codex => SINGLE_PROVIDER_SIZE,
        DisplayMode::Both => DUAL_PROVIDER_SIZE,
    }
}

/// `#[tauri::command]` The frontend's PIN button pins/releases the panel.
#[tauri::command]
pub fn set_pinned(state: tauri::State<'_, PinnedState>, value: bool) {
    *lock(&state) = value;
}

#[tauri::command]
pub fn set_display_mode(
    app: AppHandle,
    mode: DisplayMode,
    transition: tauri::State<'_, DisplayModeTransition>,
    auto_guard: tauri::State<'_, AutoRepositionGuard>,
) -> Result<(), String> {
    let _transition = lock(&transition);
    let window = app
        .get_webview_window("cluster")
        .ok_or_else(|| "cluster window is unavailable".to_string())?;
    let previous_size = window
        .outer_size()
        .map_err(|error| format!("read window size: {error}"))?;
    let previous_placement = if platform_positioning_supported() {
        Some((
            window
                .outer_position()
                .map_err(|error| format!("read window position: {error}"))?,
            window
                .scale_factor()
                .map_err(|error| format!("read window scale: {error}"))?,
        ))
    } else {
        None
    };
    let (width, height) = display_mode_size(mode);
    window
        .set_size(LogicalSize::new(width, height))
        .map_err(|error| format!("resize window: {error}"))?;
    if let Some((current, previous_scale)) = previous_placement {
        if let Err(error) = position_at(
            &window,
            f64::from(current.x),
            f64::from(current.y),
            &auto_guard,
        ) {
            let size_rollback = window.set_size(previous_size);
            let position_rollback = set_top_left(
                &window,
                f64::from(current.x),
                f64::from(current.y),
                previous_scale,
            );
            return match (size_rollback, position_rollback) {
                (Ok(()), Ok(())) => Err(error),
                (size_result, position_result) => Err(format!(
                    "{error}; rollback failed (size: {}; position: {})",
                    size_result
                        .err()
                        .map(|value| value.to_string())
                        .unwrap_or_else(|| "ok".into()),
                    position_result.err().unwrap_or_else(|| "ok".into())
                )),
            };
        }
    }
    Ok(())
}

// ─────────────────────────────────────────────────────────────────────────────
// D41: drag-to-move with a persisted override
// ─────────────────────────────────────────────────────────────────────────────

/// User-dragged position override (physical pixels), if any. `None` means
/// the default behavior applies: anchored under the tray icon on every open.
pub type PositionState = Arc<Mutex<Option<(f64, f64)>>>;

/// Last physical cursor position observed over the tray icon. Kept separate
/// from [`PositionState`]: this is an automatic anchor, not a user drag
/// override, and may be replaced on every tray click.
pub struct TrayAnchorState(pub Arc<Mutex<Option<(f64, f64)>>>);

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

fn tray_anchor_file(app: &AppHandle) -> Option<std::path::PathBuf> {
    app.path()
        .app_data_dir()
        .ok()
        .map(|dir| dir.join("tray-anchor.json"))
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

pub fn load_tray_anchor(app: &AppHandle) -> TrayAnchorState {
    let anchor = tray_anchor_file(app)
        .and_then(|path| fs::read_to_string(path).ok())
        .and_then(|data| serde_json::from_str::<StoredPosition>(&data).ok())
        .map(|stored| (stored.x, stored.y));
    TrayAnchorState(Arc::new(Mutex::new(anchor)))
}

pub fn record_tray_anchor(app: &AppHandle, state: &TrayAnchorState, x: f64, y: f64) {
    *lock(&state.0) = Some((x, y));
    let Some(path) = tray_anchor_file(app) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(&StoredPosition { x, y }) {
        let _ = fs::write(path, json);
    }
}

/// Converts the last observed tray cursor into a zero-size physical anchor.
/// Stale coordinates from a removed/rearranged monitor are discarded.
pub fn observed_tray_rect(
    app: &AppHandle,
    window: &WebviewWindow,
    state: &TrayAnchorState,
) -> Option<Rect> {
    let (x, y) = (*lock(&state.0))?;
    let on_screen = window.available_monitors().ok()?.iter().any(|monitor| {
        let position = monitor.position();
        let size = monitor.size();
        f64::from(position.x) <= x
            && x <= f64::from(position.x) + f64::from(size.width)
            && f64::from(position.y) <= y
            && y <= f64::from(position.y) + f64::from(size.height)
    });
    if !on_screen {
        *lock(&state.0) = None;
        if let Some(path) = tray_anchor_file(app) {
            let _ = fs::remove_file(path);
        }
        return None;
    }
    Some(Rect {
        position: tauri::PhysicalPosition::new(x, y).into(),
        size: tauri::PhysicalSize::new(0.0, 0.0).into(),
    })
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
    let tray_rect = valid_tray_rect(&tray).or_else(|| {
        app.try_state::<TrayAnchorState>()
            .and_then(|state| observed_tray_rect(app, &window, &state))
    });
    position_saved_or_under_tray(app, &window, None, tray_rect.as_ref(), state, auto_guard);
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
/// unrelated, already-open panel), if the auto-show setting is off, or if
/// `PositionState`/`AutoRepositionGuard` aren't managed yet (only reachable
/// in the ~impossible case of a hook connecting within milliseconds of app
/// startup, before `.setup()` finishes).
pub fn show_for_permission(app: &AppHandle) {
    if let Some(auto_show) = app.try_state::<AutoShowOnPermissionState>() {
        if !*lock(&auto_show.0) {
            return;
        }
    }
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
    let tray_rect = app
        .try_state::<TrayIcon>()
        .and_then(|tray| valid_tray_rect(&tray))
        .or_else(|| {
            app.try_state::<TrayAnchorState>()
                .and_then(|state| observed_tray_rect(app, &window, &state))
        });
    position_saved_or_under_tray(
        app,
        &window,
        saved,
        tray_rect.as_ref(),
        &position_state,
        &auto_guard,
    );
    let _ = show_panel(&window);
    if let Some(auto_opened) = app.try_state::<AutoOpenedByPermissionState>() {
        *lock(&auto_opened.0) = true;
    }
}

/// Clears the auto-opened marker — called wherever the user takes explicit,
/// manual control of the panel's visibility (currently: any tray-icon click)
/// so a later permission resolution no longer assumes it's safe to hide a
/// window the user, not the notification, is now driving.
pub fn clear_auto_opened_by_permission(app: &AppHandle) {
    if let Some(state) = app.try_state::<AutoOpenedByPermissionState>() {
        *lock(&state.0) = false;
    }
}

/// Hides the panel once every permission notification has finished resolving
/// — queue emptied by a click, an Always Allow, or the backend's own
/// give-up timeout — but only when every guard holds: the panel's current
/// visibility is solely because THIS notification auto-opened it (not
/// because the user already had it open, or has since grabbed manual
/// control via the tray), the auto-show setting is still on, and PIN isn't
/// active. Consumes the auto-opened marker unconditionally so a later
/// PIN/setting change can't retroactively trigger a surprise close.
pub fn maybe_close_after_permission(app: &AppHandle) {
    let Some(auto_opened) = app.try_state::<AutoOpenedByPermissionState>() else {
        return;
    };
    let was_auto_opened = std::mem::replace(&mut *lock(&auto_opened.0), false);
    if !was_auto_opened {
        return;
    }
    if let Some(auto_show) = app.try_state::<AutoShowOnPermissionState>() {
        if !*lock(&auto_show.0) {
            return;
        }
    }
    if let Some(pinned) = app.try_state::<PinnedState>() {
        if *lock(&pinned) {
            return;
        }
    }
    if let Some(window) = app.get_webview_window("cluster") {
        let _ = window.hide();
    }
}

/// Shows the cluster and, on macOS, orders its non-activating utility window in
/// front of the current Space without activating the accessory application.
pub fn show_panel(window: &WebviewWindow) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        window.show()?;
        let window = window.clone();
        window.clone().run_on_main_thread(move || {
            let ns_window = window
                .ns_window()
                .expect("cc-autobahn: panel lost its native NSWindow");
            let native_window: &objc2_app_kit::NSWindow = unsafe { &*ns_window.cast() };
            native_window.orderFrontRegardless();
            native_window.makeKeyWindow();
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
            if platform_positioning_supported() {
                if let Some((x, y)) = *lock(&position_for_move) {
                    save_position(&app_handle, x, y);
                }
            }
            let _ = hide_window.hide();
        }
        WindowEvent::Moved(pos) => {
            if !platform_positioning_supported() {
                return;
            }
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

/// Gives Tao's existing NSWindow panel behavior without replacing its
/// Objective-C class. An `object_setClass` conversion to an unrelated NSPanel
/// subclass loses Tao/Wry behavior and can leave WKWebView content unpainted.
/// Adding the small panel method surface to TaoWindow preserves its identity,
/// KVO bookkeeping and renderer while still allowing the window into native
/// fullscreen Spaces.
#[cfg(target_os = "macos")]
fn configure_fullscreen_panel(window: &WebviewWindow) -> tauri::Result<()> {
    use std::ffi::{c_char, c_void};

    use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
    use objc2::{msg_send, sel};
    use objc2_app_kit::{NSScreenSaverWindowLevel, NSWindowCollectionBehavior, NSWindowStyleMask};

    extern "C-unwind" fn return_yes(_: &AnyObject, _: Sel) -> Bool {
        Bool::YES
    }

    unsafe extern "C" {
        fn class_addMethod(
            class: *const AnyClass,
            selector: Sel,
            implementation: *const c_void,
            types: *const c_char,
        ) -> bool;
    }

    let raw = window.ns_window()?;
    let object: &AnyObject = unsafe { &*raw.cast() };
    let class = object.class();
    let implementation = return_yes as *const c_void;
    let bool_method_types = c"c@:".as_ptr();

    // Adding an existing method returns false, which is harmless on repeated
    // setup. Do not replace the class: WKWebView/Tao retain its real identity.
    unsafe {
        class_addMethod(
            class,
            sel!(canBecomeKeyWindow),
            implementation,
            bool_method_types,
        );
        class_addMethod(
            class,
            sel!(canBecomeMainWindow),
            implementation,
            bool_method_types,
        );
        class_addMethod(
            class,
            sel!(_isNonactivatingPanel),
            implementation,
            bool_method_types,
        );
    }

    let native_window: &objc2_app_kit::NSWindow = unsafe { &*raw.cast() };
    native_window.setStyleMask(native_window.styleMask() | NSWindowStyleMask::NonactivatingPanel);
    native_window.setHidesOnDeactivate(false);
    native_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    native_window.setLevel(NSScreenSaverWindowLevel);

    // NonactivatingPanel normally sets this WindowServer tag during NSPanel
    // initialization. Tao created an NSWindow, so apply the equivalent private
    // AppKit setter after adding `_isNonactivatingPanel`.
    unsafe {
        let _: () = msg_send![native_window, _setPreventsActivation: Bool::YES];
    }

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
    let tray_geometry = |scale: f64| {
        // `tray-icon` already converts AppKit's bottom-left coordinates to
        // top-left screen coordinates on macOS. Converting the Y axis again
        // here places the panel outside the visible monitor.
        let pos = tray_rect.position.to_physical::<f64>(scale);
        let size = tray_rect.size.to_physical::<f64>(scale);
        (pos, size)
    };
    let find_host = |scale: f64| {
        let (pos, size) = tray_geometry(scale);
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

    let (tray_pos, tray_size) = tray_geometry(scale);
    let tray_center_x = tray_pos.x + tray_size.width / 2.0;
    let mut x = tray_center_x - (win_size.width as f64) / 2.0;
    let mut y = tray_pos.y + tray_size.height + PANEL_GAP;

    if let Some(m) = host {
        let mp = m.position();
        let ms = m.size();
        let work_area = m.work_area();
        let work_top = work_area.position.y as f64;
        let work_bottom = work_top + work_area.size.height as f64;
        let min_x = mp.x as f64 + PANEL_GAP;
        let max_x = mp.x as f64 + ms.width as f64 - win_size.width as f64 - PANEL_GAP;
        x = x.clamp(min_x, max_x.max(min_x));
        // Some macOS versions report a zero-height tray rect. The work area
        // still gives the exact first visible row below the menu bar.
        y = panel_y_below_tray(tray_pos.y, tray_size.height, work_top);
        let max_y = work_bottom - win_size.height as f64 - PANEL_GAP;
        y = y.clamp(work_top + PANEL_GAP, max_y.max(work_top + PANEL_GAP));
    }

    *lock(auto_guard) = Instant::now();
    let _ = set_top_left(window, x, y, scale);
}

/// Places the panel at a saved drag override (D41), clamped to whichever
/// monitor currently contains that point — the saved spot may no longer be
/// on screen if a monitor was disconnected since it was dragged there.
///
/// Returns `Ok(true)` if the point still falls inside a currently connected
/// monitor, `Ok(false)` if it was orphaned (no monitor contains it, so it
/// was clamped into a corner of a fallback monitor instead) — callers use
/// that to decide whether the saved override is still trustworthy, instead
/// of silently re-showing a stale, oddly clamped position on every launch
/// (D-review: this is what previously made "Reset position" necessary again
/// after every full quit/reopen once the override went stale).
pub fn position_at(
    window: &WebviewWindow,
    x: f64,
    y: f64,
    auto_guard: &AutoRepositionGuard,
) -> Result<bool, String> {
    if !platform_positioning_supported() {
        return Err("window positioning is unavailable in a native Wayland session".into());
    }
    let win_size = window
        .outer_size()
        .map_err(|error| format!("read window size: {error}"))?;
    let monitors = window
        .available_monitors()
        .map_err(|error| format!("read monitors: {error}"))?;
    if monitors.is_empty() {
        return Err("no monitor is available for window placement".into());
    }

    let exact_host = monitors.iter().find(|m| {
        let mp = m.position();
        let ms = m.size();
        (mp.x as f64) <= x
            && x <= mp.x as f64 + ms.width as f64
            && (mp.y as f64) <= y
            && y <= mp.y as f64 + ms.height as f64
    });
    let orphaned = exact_host.is_none();
    // Monitor removal: clamp an orphaned saved point to the primary monitor
    // instead of preserving an invisible coordinate forever.
    let host = exact_host.or_else(|| monitors.first());
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
    set_top_left(window, clamped_x, clamped_y, scale)?;
    Ok(!orphaned)
}

/// Reads the live tray frame once and rejects AppKit's unlaid-out initial
/// frame. Polling from a tray/main-thread callback cannot help: sleeping there
/// prevents the event loop from performing the layout that would change it.
pub fn valid_tray_rect(tray: &TrayIcon) -> Option<Rect> {
    if let Some(rect) = tray.rect().ok().flatten() {
        let size = rect.size.to_logical::<f64>(1.0);
        if size.width > 0.0 && size.height > 0.0 {
            return Some(rect);
        }
    }

    None
}

/// Restores the saved drag override if it's still valid for a currently
/// connected monitor; otherwise treats it as stale (e.g. the monitor it was
/// dragged on got disconnected), clears it from memory and disk, and falls
/// back to anchoring under the tray icon — self-healing instead of leaving
/// the panel clamped into a corner on every future launch until the user
/// notices and hits "Reset position" themselves.
pub fn position_saved_or_under_tray(
    app: &AppHandle,
    window: &WebviewWindow,
    saved: Option<(f64, f64)>,
    tray_rect: Option<&Rect>,
    position_state: &PositionState,
    auto_guard: &AutoRepositionGuard,
) {
    // Native Wayland deliberately lets the compositor choose the position.
    // set_position is unsupported there, and persisting compositor-reported
    // coordinates would create a preference that can never be restored.
    if !platform_positioning_supported() {
        return;
    }
    if let Some((x, y)) = saved {
        match position_at(window, x, y, auto_guard) {
            Ok(true) => return,
            Ok(false) => clear_position(app, position_state),
            Err(error) => {
                eprintln!("cc-autobahn: could not restore saved position: {error}");
            }
        }
    }
    if let Some(rect) = tray_rect {
        position_under_tray(window, rect, auto_guard);
    } else {
        position_at_menu_bar_fallback(window, auto_guard);
    }
}

/// Keeps cold-launch feedback visible when macOS exposes no usable tray frame.
/// A later tray click reanchors exactly from the click's cursor position.
fn position_at_menu_bar_fallback(window: &WebviewWindow, auto_guard: &AutoRepositionGuard) {
    let Ok(win_size) = window.outer_size() else {
        return;
    };
    let monitor = window
        .primary_monitor()
        .ok()
        .flatten()
        .or_else(|| window.available_monitors().ok()?.into_iter().next());
    let Some(monitor) = monitor else {
        return;
    };
    let work = monitor.work_area();
    let x = f64::from(work.position.x) + f64::from(work.size.width)
        - f64::from(win_size.width)
        - PANEL_GAP;
    let y = f64::from(work.position.y) + PANEL_GAP;
    *lock(auto_guard) = Instant::now();
    let _ = set_top_left(window, x, y, monitor.scale_factor());
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
fn set_top_left(window: &WebviewWindow, x: f64, y: f64, scale: f64) -> Result<(), String> {
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

    if native.is_some() {
        return Ok(());
    }
    window
        .set_position(PhysicalPosition::new(x as i32, y as i32))
        .map_err(|error| format!("set window position: {error}"))
}

/// Non-macOS platforms go through tao's positioning, which is correct there.
#[cfg(not(target_os = "macos"))]
fn set_top_left(window: &WebviewWindow, x: f64, y: f64, _scale: f64) -> Result<(), String> {
    window
        .set_position(PhysicalPosition::new(x as i32, y as i32))
        .map_err(|error| format!("set window position: {error}"))
}

#[cfg(target_os = "linux")]
fn platform_positioning_supported() -> bool {
    if let Some(backend) = crate::env_lock::var_os("GDK_BACKEND") {
        let backend = backend.to_string_lossy().to_ascii_lowercase();
        if backend.split(',').next() == Some("x11") {
            return true;
        }
        if backend.split(',').next() == Some("wayland") {
            return false;
        }
    }
    !crate::env_lock::var_os("XDG_SESSION_TYPE")
        .is_some_and(|value| value.to_string_lossy().eq_ignore_ascii_case("wayland"))
        && crate::env_lock::var_os("WAYLAND_DISPLAY").is_none()
}

#[cfg(not(target_os = "linux"))]
fn platform_positioning_supported() -> bool {
    true
}

#[cfg(test)]
mod display_mode_tests {
    use super::*;

    #[test]
    fn single_modes_preserve_the_legacy_panel_size() {
        assert_eq!(display_mode_size(DisplayMode::Claude), (550.0, 150.0));
        assert_eq!(display_mode_size(DisplayMode::Codex), (550.0, 150.0));
    }

    #[test]
    fn both_mode_only_grows_vertically() {
        let single = display_mode_size(DisplayMode::Claude);
        let both = display_mode_size(DisplayMode::Both);
        assert_eq!(both.0, single.0);
        assert!(both.1 > single.1);
        assert_eq!(both, (550.0, 290.0));
    }

    #[test]
    fn zero_height_tray_uses_visible_work_area() {
        assert_eq!(panel_y_below_tray(0.0, 0.0, 68.0), 72.0);
        assert_eq!(panel_y_below_tray(10.0, 58.0, 68.0), 72.0);
    }
}
