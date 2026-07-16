// cc-autobahn — Tauri shell entrypoint.
//
// Boots the frameless, always-on-top cluster window (tauri.conf.json) and
// starts two sensors on dedicated threads:
//   · engine (engine.rs): detects ccusage, polls `blocks --active --json`
//     every 15 s → cost/proyección/autonomía.
//   · burn   (burn.rs):   tails the active session JSONL → tok/s por respuesta
//     → `burn-tick`. La aguja del velocímetro (D8).
// The statusline sensor (rate_limits oficiales) lands in a later pass.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod burn;
mod engine;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            engine::start(handle.clone());
            burn::start(handle);
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("cc-autobahn: error while running the cluster");
}
