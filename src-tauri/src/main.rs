// cc-autobahn — Tauri shell entrypoint.
//
// Triple binary:
//   · `cc-autobahn statusline` → Claude Code statusLine mode (D12): reads the
//     session JSON from stdin, re-emits the user's previous line (chain), and
//     dumps the JSON to a file that the window tails. Resolved BEFORE
//     building the webview → no GUI, fast exit.
//   · `cc-autobahn permission-hook` → Claude Code PermissionRequest hook mode
//     (D42): reads the request from stdin, blocks on the GUI's socket for a
//     human decision, prints the decision (or nothing, fail-open). Same
//     no-GUI, fast-exit shape as statusline mode.
//   · no args → GUI mode: menu-bar icon (D24) + anchored panel + four
//     sensors on dedicated threads.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod burn;
mod engine;
mod env_lock;
mod path_state;
mod pathfix;
mod permission;
pub mod providers;
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
    match args.next().as_deref() {
        Some("statusline") => {
            sensor::run_statusline();
            return;
        }
        Some("permission-hook") => {
            permission::hook_bin::run_permission_hook();
            return;
        }
        _ => {}
    }

    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            engine::engine_status,
            engine::install::install_bun,
            engine::history::history_daily,
            sensor::install::sensor_status,
            sensor::sensor_snapshot,
            sensor::install::sensor_preview_install,
            sensor::install::install_sensor,
            sensor::install::uninstall_sensor,
            permission::permission_approve,
            permission::permission_approve_always,
            permission::permission_deny,
            permission::permission_pending_snapshot,
            providers::provider_health_snapshot,
            permission::install::permission_status,
            permission::install::permission_preview_install,
            permission::install::install_permission_hook,
            permission::install::uninstall_permission_hook,
            window::set_pinned,
            window::reset_position,
            tray_icon::set_tray_alert,
        ])
        .manage::<PinnedState>(Arc::new(Mutex::new(false)))
        .manage::<PathState>(Arc::new(Mutex::new(None)))
        .manage::<providers::ProviderHealthState>(providers::new_health_state())
        .setup(|app| {
            pathfix::apply(&app.handle().clone());
            sensor::install::refresh_if_stale();
            permission::install::refresh_if_stale();

            let handle = app.handle().clone();
            providers::start_enabled(handle.clone());

            // No icon in Dock/Cmd+Tab (D24): lives only in the menu bar.
            #[cfg(target_os = "macos")]
            app.handle()
                .set_activation_policy(tauri::ActivationPolicy::Accessory)?;

            let win = app
                .get_webview_window("cluster")
                .expect("cc-autobahn: missing the 'cluster' window declared in tauri.conf.json");

            // D41: restores the drag-to-move override, if the user left one
            // saved from a previous session; `None` keeps the D24 default
            // (anchored under the tray icon).
            let position_state: window::PositionState =
                Arc::new(Mutex::new(window::load_position(&app.handle().clone())));
            app.manage(position_state.clone());

            let (last_blur_hide, auto_reposition_guard) =
                window::wire(app, &win, position_state.clone())?;
            app.manage(auto_reposition_guard.clone());

            // Started only after PositionState/AutoRepositionGuard are
            // managed (D42 review fix): its socket listener can accept a
            // connection and call `window::show_for_permission` almost
            // immediately, and that function's `try_state` lookups silently
            // no-op if either isn't managed yet.
            permission::start(handle);

            let tray_handle =
                tray::build(app, last_blur_hide, auto_reposition_guard, position_state)?;

            // Initial state: full ring (100%) until the first real data from
            // engine::poll or sensor::tail (D-review: tray icon as a
            // progress ring instead of a static disc with no information).
            app.manage(tray_handle);
            tray_icon::set_progress(
                &app.handle().clone(),
                100.0,
                tray_icon::ProgressSource::Estimated,
            );

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("cc-autobahn: error while running the cluster");
}
