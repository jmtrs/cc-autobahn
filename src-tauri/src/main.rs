// cc-autobahn — Tauri shell entrypoint.
//
// Binario dual:
//   · `cc-autobahn statusline` → modo statusLine de Claude Code (D12): lee el
//     JSON de sesión por stdin, reemite la línea previa del usuario (chain) y
//     vuelca el JSON a un fichero que tailea la ventana. Se resuelve ANTES de
//     construir la webview → sin GUI, salida rápida.
//   · sin args → modo GUI: icono de menu-bar (D24) + panel anclado + tres
//     sensores en hilos dedicados.
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
// Clicar el icono para *cerrar* el panel dispara primero el blur (que oculta)
// y después el evento de click del tray (que reabriría). Si el click llega
// justo tras un hide-por-blur, se ignora (D24).
const REOPEN_GUARD: Duration = Duration::from_millis(300);

/// Estado del botón PIN (frontend): si está activo, el hide-on-blur no oculta
/// el panel al perder el foco.
type PinnedState = Arc<Mutex<bool>>;

/// `#[tauri::command]` El botón PIN del frontend fija/libera el panel.
#[tauri::command]
fn set_pinned(state: tauri::State<'_, PinnedState>, value: bool) {
    *state.lock().unwrap() = value;
}

fn main() {
    // Modo statusline: se decide antes de tocar Tauri (sin webview, sin ventana).
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

            // Sin icono en Dock/Cmd+Tab (D24): vive solo en la barra de menú.
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;

            let window = app
                .get_webview_window("cluster")
                .expect("cc-autobahn: falta la ventana 'cluster' declarada en tauri.conf.json");

            // Esquinas nativas redondeadas (D24 addendum): con transparent:true,
            // Tauri/WebKit no clipea bien el CSS border-radius al alpha de la
            // ventana (bug conocido, deja un "pico" cuadrado en las 4 esquinas).
            // Se clipea el NSWindow a nivel de CALayer, que sí antialiasea bien.
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
                        return; // PIN activo (D24): no ocultar al perder el foco.
                    }
                    *blur_flag.lock().unwrap() = Some(Instant::now());
                    let _ = hide_window.hide();
                }
            });

            let quit_item = MenuItemBuilder::with_id("quit", "Salir de cc-autobahn").build(app)?;
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

            // Estado inicial: anillo lleno (100%) hasta el primer dato real de
            // engine::poll o sensor::tail (D-review: icono de bandeja como
            // anillo de progreso en vez de disco estático sin información).
            app.manage(tray);
            tray_icon::set_progress(&app.handle().clone(), 100.0);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("cc-autobahn: error while running the cluster");
}

/// Ancla el panel justo debajo del icono de tray, centrado horizontalmente y
/// acotado al monitor que contiene el icono para no salirse de pantalla.
fn position_under_tray(window: &WebviewWindow, tray_rect: &Rect) {
    let Ok(win_size) = window.outer_size() else {
        return;
    };
    let monitors = window.available_monitors().unwrap_or_default();

    // Encuentra el monitor que contiene el centro del icono, convirtiendo el
    // rect con una escala dada (closure para poder reintentar con la escala
    // correcta abajo — D-review: la escala de LA VENTANA no es fiable si el
    // tray vive en un monitor con DPI distinto en setups multi-monitor).
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

    // Primera pasada: escala de la ventana, solo para UBICAR el monitor.
    let guess_scale = window.scale_factor().unwrap_or(1.0);
    // Segunda pasada: si el monitor tiene su propia escala, se usa esa para
    // el cálculo definitivo (coincide con la de la ventana en el caso común
    // de un solo monitor o DPI uniforme).
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
