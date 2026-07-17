//! "INSTALL ENGINE" button (D9, Phase 4): runs Bun's official installer when
//! no engine (ccusage/npx/bunx) is on PATH.

use std::path::{Path, PathBuf};
use std::process::Command;

use tauri::{AppHandle, Emitter};

use super::{detect, start};

/// `#[tauri::command]` "INSTALL ENGINE" button (D9, Phase 4): runs Bun's
/// official installer, updates the `PATH` of the already-running process (the
/// installer only appends it to the shell rc, which this process never
/// re-reads) and retries the engine.
///
/// Fire-and-forget by design (D36-review): a plain synchronous `#[tauri::command]`
/// runs on the same thread that pumps the webview's event loop — a `curl | bash`
/// child process blocking that thread for 10s+ freezes the whole UI (button
/// label swap, spinner CSS, everything), a classic Tauri footgun. The heavy
/// work moves to `std::thread::spawn` (same "never block the UI" rule as
/// `engine`/`burn`/`sensor`); the outcome comes back via `install-succeeded`/
/// `install-failed` events instead of the invoke's return value.
#[tauri::command]
pub fn install_bun(app: AppHandle) {
    if let Some(engine) = detect() {
        start(app.clone());
        let _ = app.emit("install-succeeded", engine.label());
        return;
    }

    std::thread::spawn(move || {
        let _ = app.emit("install-progress", "downloading");
        if let Err(e) = run_bun_installer() {
            let _ = app.emit("install-failed", e);
            return;
        }

        let _ = app.emit("install-progress", "detecting");
        if let Some(dir) = bun_bin_dir() {
            prepend_path(&dir);
        }

        match detect() {
            Some(engine) => {
                start(app.clone());
                let _ = app.emit("install-succeeded", engine.label());
            }
            None => {
                let _ = app.emit("install-failed", "Bun was installed but bunx isn't on PATH");
            }
        }
    });
}

/// `~/.bun/bin`, the fixed destination of the official installer.
#[cfg(unix)]
fn bun_bin_dir() -> Option<PathBuf> {
    let home = crate::env_lock::var_os("HOME")?;
    Some(PathBuf::from(home).join(".bun").join("bin"))
}

#[cfg(not(unix))]
fn bun_bin_dir() -> Option<PathBuf> {
    None
}

/// Prepends `dir` to the current process's `PATH` (not the shell's) so that
/// `on_path` and subsequent `Command`s find `bunx` without restarting the app.
fn prepend_path(dir: &Path) {
    let existing = crate::env_lock::var_os("PATH").unwrap_or_default();
    let mut paths = vec![dir.to_path_buf()];
    paths.extend(std::env::split_paths(&existing));
    if let Ok(joined) = std::env::join_paths(paths) {
        crate::env_lock::set_var("PATH", joined);
    }
}

/// Official Bun installer (https://bun.sh/install). macOS/Linux only for
/// now — the rest of the project isn't tested on Windows either (D24).
#[cfg(unix)]
fn run_bun_installer() -> Result<(), String> {
    let status = Command::new("sh")
        .arg("-c")
        .arg("curl -fsSL https://bun.sh/install | bash")
        .status()
        .map_err(|e| format!("could not launch the Bun installer: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the Bun installer exited with {status}"))
    }
}

#[cfg(not(unix))]
fn run_bun_installer() -> Result<(), String> {
    Err("Automatic installation is only available on macOS/Linux for now. Install Bun manually from https://bun.sh and restart cc-autobahn.".to_string())
}
