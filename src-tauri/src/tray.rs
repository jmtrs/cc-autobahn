//! Menu-bar icon (D24): menu, tray icon, and the left-click show/hide toggle.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::Manager;

use crate::window::{position_at, position_under_tray, AutoRepositionGuard, PositionState};

const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray-icon-template.png");
// Clicking the icon to *close* the panel first triggers the blur (which
// hides it) and then the tray click event (which would reopen it). If the
// click arrives right after a hide-by-blur, it's ignored (D24).
const REOPEN_GUARD: Duration = Duration::from_millis(300);

/// Builds the tray menu + icon and wires the left-click show/hide toggle.
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
    let quit_item = MenuItemBuilder::with_id("quit", "Quit cc-autobahn").build(app)?;
    let tray_menu = MenuBuilder::new(app)
        .item(&reset_position_item)
        .item(&quit_item)
        .build()?;
    let tray_icon = tauri::image::Image::from_bytes(TRAY_ICON_BYTES)?;

    let position_state_for_menu = position_state.clone();
    let auto_guard_for_menu = auto_reposition_guard.clone();

    TrayIconBuilder::new()
        .icon(tray_icon)
        .icon_as_template(true)
        .menu(&tray_menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            if event.id() == "quit" {
                app.exit(0);
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
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                rect,
                ..
            } = event
            else {
                return;
            };
            let app = tray.app_handle();
            let Some(window) = app.get_webview_window("cluster") else {
                return;
            };

            let just_hid = crate::window::lock(&last_blur_hide)
                .map(|t| t.elapsed() < REOPEN_GUARD)
                .unwrap_or(false);
            if just_hid {
                return;
            }

            if window.is_visible().unwrap_or(false) {
                let _ = window.hide();
            } else {
                let saved = *crate::window::lock(&position_state);
                if let Some((x, y)) = saved {
                    position_at(&window, x, y, &auto_reposition_guard);
                } else {
                    position_under_tray(&window, &rect, &auto_reposition_guard);
                }
                let _ = window.show();
                let _ = window.set_focus();
            }
        })
        .build(app)
}
