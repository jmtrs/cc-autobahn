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

/// Cheap, harmless hardening for WebKit's DMA-BUF *compositing* path on
/// old/broken Mesa+EGL combos: disabling it falls back to plain GL texture
/// upload, still GPU-accelerated, so modern systems shouldn't take a real hit.
/// Only sets the value if the user/distro hasn't already exported one.
///
/// This does NOT fix the harder failure seen on Intel HD 4000 / Ivy Bridge +
/// Mesa >=26 (crocus), where the bundled AppImage's frozen WebKit aborts at
/// startup with `Could not create default EGL display: EGL_BAD_PARAMETER` —
/// that abort is in EGL *display* creation, before any renderer/compositing
/// mode is chosen, so no WebKit env var avoids it (all tested:
/// `WEBKIT_DISABLE_COMPOSITING_MODE`, `LIBGL_ALWAYS_SOFTWARE`,
/// `GALLIUM_DRIVER=llvmpipe`, `GDK_BACKEND=x11`). That is a bundled-WebKit
/// *version* problem (upstream bug #280239, fixed in 2.52) and is addressed at
/// packaging level by preferring the host WebKitGTK in the AppImage (D66), not
/// here.
///
/// Safety: called before `tauri::Builder::default()`, before any thread in
/// this process is spawned — no concurrent `getenv`/`setenv` race is
/// possible yet (see `env_lock.rs`/`path_state.rs` for why this crate
/// otherwise avoids mutating real process env post-startup).
#[cfg(target_os = "linux")]
fn harden_webkit_render_env() {
    if std::env::var_os("WEBKIT_DISABLE_DMABUF_RENDERER").is_none() {
        unsafe { std::env::set_var("WEBKIT_DISABLE_DMABUF_RENDERER", "1") };
    }
}

fn main() {
    // Statusline mode: decided before touching Tauri (no webview, no window).
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("statusline") => {
            sensor::run_statusline();
            return;
        }
        Some("permission-hook") => {
            let provider = match args.next().as_deref() {
                Some("codex") => providers::ProviderId::Codex,
                _ => providers::ProviderId::Claude,
            };
            permission::hook_bin::run_permission_hook(provider);
            return;
        }
        _ => {}
    }

    #[cfg(target_os = "linux")]
    harden_webkit_render_env();

    tauri::Builder::default()
        // Must be the first registered plugin. A desktop-entry launch while
        // the tray app is already running reuses that process instead of
        // creating a second tray, provider set and permission listener.
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            window::clear_auto_opened_by_permission(app);
            if let Some(window) = app.get_webview_window("cluster") {
                let _ = window::show_panel(&window);
            }
        }))
        .invoke_handler(tauri::generate_handler![
            engine::engine_status,
            engine::install::install_bun,
            engine::history::history_daily,
            engine::history::history_sessions,
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
            providers::provider_activity_snapshot,
            providers::provider_diagnostics_snapshot,
            providers::codex::app_server::codex_account_snapshot,
            providers::codex::codex_desktop_permission_snapshot,
            permission::install::permission_status,
            permission::install::permission_preview_install,
            permission::install::install_permission_hook,
            permission::install::uninstall_permission_hook,
            permission::codex_install::codex_permission_status,
            permission::codex_install::codex_permission_preview_install,
            permission::codex_install::install_codex_permission_hook,
            permission::codex_install::uninstall_codex_permission_hook,
            window::set_pinned,
            window::set_auto_show_on_permission,
            window::set_display_mode,
            window::reset_position,
            tray_icon::set_tray_alert,
        ])
        .manage::<PinnedState>(Arc::new(Mutex::new(false)))
        .manage(window::AutoShowOnPermissionState(Arc::new(Mutex::new(
            true,
        ))))
        .manage(window::AutoOpenedByPermissionState(Arc::new(Mutex::new(
            false,
        ))))
        .manage::<window::DisplayModeTransition>(Arc::new(Mutex::new(())))
        .manage::<PathState>(Arc::new(Mutex::new(None)))
        .manage::<providers::ProviderHealthState>(providers::new_health_state())
        .manage::<providers::ProviderActivityState>(providers::new_activity_state())
        .manage::<providers::codex::app_server::AccountSensorState>(
            providers::codex::app_server::new_state(),
        )
        .manage::<providers::codex::DesktopPermissionState>(
            providers::codex::new_desktop_permission_state(),
        )
        .setup(|app| {
            pathfix::apply(&app.handle().clone());
            sensor::install::refresh_if_stale();
            permission::install::refresh_if_stale();
            permission::codex_install::refresh_if_stale();

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
            app.manage(window::load_tray_anchor(app.handle()));

            let (last_blur_hide, auto_reposition_guard) =
                window::wire(app, &win, position_state.clone())?;
            app.manage(auto_reposition_guard.clone());

            // Started only after PositionState/AutoRepositionGuard are
            // managed (D42 review fix): its socket listener can accept a
            // connection and call `window::show_for_permission` almost
            // immediately, and that function's `try_state` lookups silently
            // no-op if either isn't managed yet.
            permission::start(handle);

            let tray_handle = tray::build(
                app,
                last_blur_hide,
                auto_reposition_guard.clone(),
                position_state.clone(),
            )?;

            app.manage(tray_handle);

            // A direct app launch must produce visible feedback. macOS can
            // expose a permanently degenerate NSStatusBarWindow frame; the
            // positioning helper rejects it and uses a visible top-right
            // fallback until the first click provides a real cursor anchor.
            let saved_position = *window::lock(&position_state);
            let tray = app.state::<tauri::tray::TrayIcon>();
            let tray_rect = window::valid_tray_rect(&tray).or_else(|| {
                let anchor = app.state::<window::TrayAnchorState>();
                window::observed_tray_rect(app.handle(), &win, &anchor)
            });
            window::position_saved_or_under_tray(
                app.handle(),
                &win,
                saved_position,
                tray_rect.as_ref(),
                &position_state,
                &auto_reposition_guard,
            );
            window::show_panel(&win)?;

            // Initial state: full ring (100%) until the first real data from
            // engine::poll or sensor::tail (D-review: tray icon as a
            // progress ring instead of a static disc with no information).
            tray_icon::set_progress(
                &app.handle().clone(),
                providers::ProviderId::Claude,
                100.0,
                tray_icon::ProgressSource::Estimated,
            );

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("cc-autobahn: error while building the cluster")
        .run(|_, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                providers::codex::app_server::stop();
            }
        });
}
