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
mod env_lock;
mod path_state;
mod pathfix;
mod sensor;
mod tray;
mod tray_icon;
mod window;

use std::sync::{Arc, Mutex};

use tauri::Manager;

use path_state::PathState;
use window::PinnedState;

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
            engine::install::install_bun,
            engine::history::history_daily,
            sensor::install::sensor_status,
            sensor::install::sensor_preview_install,
            sensor::install::install_sensor,
            sensor::install::uninstall_sensor,
            window::set_pinned,
            tray_icon::set_tray_alert,
        ])
        .manage::<PinnedState>(Arc::new(Mutex::new(false)))
        .manage::<PathState>(Arc::new(Mutex::new(None)))
        .setup(|app| {
            pathfix::apply(&app.handle().clone());
            sensor::install::refresh_if_stale();

            let handle = app.handle().clone();
            engine::start(handle.clone());
            burn::start(handle.clone());
            sensor::start(handle);

            // No icon in Dock/Cmd+Tab (D24): lives only in the menu bar.
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;

            let win = app
                .get_webview_window("cluster")
                .expect("cc-autobahn: missing the 'cluster' window declared in tauri.conf.json");

            let last_blur_hide = window::wire(app, &win)?;
            let tray_handle = tray::build(app, last_blur_hide)?;

            // Initial state: full ring (100%) until the first real data from
            // engine::poll or sensor::tail (D-review: tray icon as a
            // progress ring instead of a static disc with no information).
            app.manage(tray_handle);
            tray_icon::set_progress(&app.handle().clone(), 100.0);

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("cc-autobahn: error while running the cluster");
}
