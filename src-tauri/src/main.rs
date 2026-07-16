// cc-autobahn — Tauri shell entrypoint.
//
// Dual binary:
//   · `cc-autobahn statusline` → Claude Code statusLine mode (D12): reads the
//     session JSON from stdin, re-emits the user's previous line (chain), and
//     dumps the JSON to a file that the window tails. Resolved BEFORE
//     building the webview → no GUI, fast exit.
//   · no args → GUI mode: menu-bar icon (D24) + anchored panel + three
//     sensors on dedicated threads.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod burn;
mod engine;
mod sensor;
mod tray_icon;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tauri::menu::{MenuBuilder, MenuItemBuilder};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, PhysicalPosition, Rect, WebviewWindow, WindowEvent};

const TRAY_ICON_BYTES: &[u8] = include_bytes!("../icons/tray-icon-template.png");
const PANEL_GAP: f64 = 4.0;
// Clicking the icon to *close* the panel first triggers the blur (which
// hides it) and then the tray click event (which would reopen it). If the
// click arrives right after a hide-by-blur, it's ignored (D24).
const REOPEN_GUARD: Duration = Duration::from_millis(300);

/// PIN button state (frontend): if active, hide-on-blur doesn't hide
/// the panel when it loses focus.
type PinnedState = Arc<Mutex<bool>>;

/// `#[tauri::command]` The frontend's PIN button pins/releases the panel.
#[tauri::command]
fn set_pinned(state: tauri::State<'_, PinnedState>, value: bool) {
    *state.lock().unwrap() = value;
}

fn main() {
    // Statusline mode: decided before touching Tauri (no webview, no window).
    let mut args = std::env::args().skip(1);
    if args.next().as_deref() == Some("statusline") {
        sensor::run_statusline();
        return;
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            engine::engine_status,
            engine::install_bun,
            sensor::sensor_status,
            sensor::sensor_preview_install,
            sensor::install_sensor,
            sensor::uninstall_sensor,
            set_pinned,
        ])
        .manage::<PinnedState>(Arc::new(Mutex::new(false)))
        .setup(|app| {
            let handle = app.handle().clone();
            engine::start(handle.clone());
            burn::start(handle.clone());
            sensor::start(handle);

            // No icon in Dock/Cmd+Tab (D24): lives only in the menu bar.
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;

            let window = app
                .get_webview_window("cluster")
                .expect("cc-autobahn: missing the 'cluster' window declared in tauri.conf.json");

            // Native rounded corners (D24 addendum): with transparent:true,
            // Tauri/WebKit doesn't clip the CSS border-radius to the window's
            // alpha well (known bug, leaves a square "corner" on all 4 corners).
            // The NSWindow is clipped at the CALayer level, which antialiases correctly.
            #[cfg(target_os = "macos")]
            {
                let ns_window: &objc2_app_kit::NSWindow =
                    unsafe { &*window.ns_window()?.cast() };
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
                    if *pinned_for_blur.lock().unwrap() {
                        return; // PIN active (D24): don't hide on losing focus.
                    }
                    *blur_flag.lock().unwrap() = Some(Instant::now());
                    let _ = hide_window.hide();
                }
            });

            let quit_item = MenuItemBuilder::with_id("quit", "Quit cc-autobahn").build(app)?;
            let tray_menu = MenuBuilder::new(app).item(&quit_item).build()?;
            let tray_icon = tauri::image::Image::from_bytes(TRAY_ICON_BYTES)?;

            let tray = TrayIconBuilder::new()
                .icon(tray_icon)
                .icon_as_template(true)
                .menu(&tray_menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| {
                    if event.id() == "quit" {
                        app.exit(0);
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

                    let just_hid = last_blur_hide
                        .lock()
                        .unwrap()
                        .map(|t| t.elapsed() < REOPEN_GUARD)
                        .unwrap_or(false);
                    if just_hid {
                        return;
                    }

                    if window.is_visible().unwrap_or(false) {
                        let _ = window.hide();
                    } else {
                        position_under_tray(&window, &rect);
                        let _ = window.show();
                        let _ = window.set_focus();
                    }
                })
                .build(app)?;

            // Initial state: full ring (100%) until the first real data from
            // engine::poll or sensor::tail (D-review: tray icon as a
            // progress ring instead of a static disc with no information).
            app.manage(tray);
            tray_icon::set_progress(&app.handle().clone(), 100.0);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("cc-autobahn: error while running the cluster");
}

/// Anchors the panel right below the tray icon, centered horizontally and
/// clamped to the monitor that contains the icon so it doesn't go off-screen.
fn position_under_tray(window: &WebviewWindow, tray_rect: &Rect) {
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
