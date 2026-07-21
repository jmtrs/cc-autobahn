//! Menu-bar icon (D24): native menu plus macOS click / Linux menu visibility controls.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, PhysicalSize, Rect};

use crate::window::{
    position_saved_or_under_tray, show_panel, valid_tray_rect, AutoRepositionGuard, PositionState,
};

// macOS uses the alpha-only "template" PNG so AppKit tints it per light/dark
// mode (D24). Linux/AppIndicator has no template concept — that PNG renders
// solid black — so Linux embeds an amber VFD disc instead (D55). The first
// `tray_icon::set_progress` call overwrites this within milliseconds either
// way; this only avoids a black-square flash at cold launch.
#[cfg(target_os = "macos")]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray-icon-template.png");
#[cfg(not(target_os = "macos"))]
const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray-icon-linux.png");
// Clicking the icon to *close* the panel first triggers the blur (which
// hides it) and then the tray click event (which would reopen it). If the
// click arrives right after a hide-by-blur, it's ignored (D24).
const REOPEN_GUARD: Duration = Duration::from_millis(300);

/// Builds the tray menu + icon and wires platform-appropriate show/hide controls.
/// `last_blur_hide` (from `window::wire`) debounces the reopen-after-blur
/// click; `auto_reposition_guard` and `position_state` implement the D41
/// drag-to-move override (default anchor under the tray unless the user has
/// dragged the panel elsewhere, resettable from the menu).
pub fn build(
    app: &tauri::App,
    last_blur_hide: Arc<Mutex<Option<Instant>>>,
    auto_reposition_guard: AutoRepositionGuard,
    position_state: PositionState,
) -> tauri::Result<TrayIcon> {
    let reset_position_item =
        MenuItemBuilder::with_id("reset_position", "Reset position").build(app)?;
    let toggle_panel_item =
        MenuItemBuilder::with_id("toggle_panel", "Show / hide cc-autobahn").build(app)?;
    let quit_item = MenuItemBuilder::with_id("quit", "Quit CC Autobahn").build(app)?;
    let tray_menu = MenuBuilder::new(app)
        .item(&toggle_panel_item)
        .item(&reset_position_item)
        .item(&quit_item)
        .build()?;
    let tray_icon = tauri::image::Image::from_bytes(TRAY_ICON_BYTES)?;

    let position_state_for_menu = position_state.clone();
    let auto_guard_for_menu = auto_reposition_guard.clone();

    let tray_builder = TrayIconBuilder::new()
        .icon(tray_icon)
        .icon_as_template(true)
        .menu(&tray_menu)
        .on_menu_event(move |app, event| {
            if event.id() == "quit" {
                app.exit(0);
            } else if event.id() == "toggle_panel" {
                toggle_panel(app, &position_state_for_menu, &auto_guard_for_menu, None);
            } else if event.id() == "reset_position" {
                crate::window::reset_position_now(
                    app,
                    &position_state_for_menu,
                    &auto_guard_for_menu,
                );
            }
        })
        .on_tray_icon_event(move |tray, event| {
            let TrayIconEvent::Click {
                button,
                button_state,
                position,
                ..
            } = event
            else {
                return;
            };
            let app = tray.app_handle();
            let Some(window) = app.get_webview_window("cluster") else {
                return;
            };

            if let Some(anchor) = app.try_state::<crate::window::TrayAnchorState>() {
                crate::window::record_tray_anchor(app, &anchor, position.x, position.y);
            }

            if button == MouseButton::Right && button_state == MouseButtonState::Down {
                // This callback runs before tray-icon's own click handling
                // decides whether to pop the native "Reset position"/"Quit"
                // menu, so hiding here beats it there. The panel otherwise
                // sits above that menu (D43's NSScreenSaverWindowLevel is
                // higher than a native context menu's level) and the menu
                // looks like it never opened.
                let _ = window.hide();
                return;
            }
            if button != MouseButton::Left || button_state != MouseButtonState::Up {
                return;
            }

            let just_hid = crate::window::lock(&last_blur_hide)
                .map(|t| t.elapsed() < REOPEN_GUARD)
                .unwrap_or(false);
            if just_hid {
                return;
            }

            if window.is_visible().unwrap_or(false) {
                toggle_panel(app, &position_state, &auto_reposition_guard, None);
            } else {
                // Never trust the click event's own `rect`: on cold launch it
                // can carry the NSStatusItem's degenerate initial frame. Query
                // the live frame once and reject it if AppKit still reports an
                // empty size. Retrying here only blocks AppKit's event loop and
                // cannot make layout advance.
                let fresh_rect = valid_tray_rect(tray).unwrap_or_else(|| Rect {
                    // The click cursor is the only screen coordinate AppKit
                    // reports correctly when NSStatusBarWindow.frame stays at
                    // its degenerate bootstrap value. A zero-size anchor centers
                    // the panel on the actual click and lets the monitor work
                    // area supply the menu-bar bottom edge.
                    position: position.into(),
                    size: PhysicalSize::new(0.0, 0.0).into(),
                });
                toggle_panel(
                    app,
                    &position_state,
                    &auto_reposition_guard,
                    Some(&fresh_rect),
                );
            }
        });

    // Tauri does not emit TrayIconEvent or support menu-on-left-click on Linux.
    // Its right-click context menu therefore carries the explicit toggle item.
    // macOS keeps the direct left-click toggle and right-click menu.
    #[cfg(target_os = "macos")]
    let tray_builder = tray_builder.show_menu_on_left_click(false);

    tray_builder.build(app)
}

fn toggle_panel(
    app: &AppHandle,
    position_state: &PositionState,
    auto_guard: &AutoRepositionGuard,
    tray_rect: Option<&Rect>,
) {
    let Some(window) = app.get_webview_window("cluster") else {
        return;
    };
    crate::window::clear_auto_opened_by_permission(app);
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }

    let saved = *crate::window::lock(position_state);
    position_saved_or_under_tray(app, &window, saved, tray_rect, position_state, auto_guard);
    if let Err(error) = show_panel(&window) {
        eprintln!("cc-autobahn: could not show tray panel: {error}");
    }
}
