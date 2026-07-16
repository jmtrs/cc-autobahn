//! "INSTALL ENGINE" button (D9, Phase 4): runs Bun's official installer when
//! no engine (ccusage/npx/bunx) is on PATH.

use std::path::{Path, PathBuf};
use std::process::Command;

use tauri::AppHandle;

use super::{detect, start};

/// `#[tauri::command]` "INSTALL ENGINE" button (D9, Phase 4): runs Bun's
/// official installer, updates the `PATH` of the already-running process (the
/// installer only appends it to the shell rc, which this process never
/// re-reads) and retries the engine. `Err` with a readable message to render in the overlay.
#[tauri::command]
pub fn install_bun(app: AppHandle) -> Result<String, String> {
    if let Some(engine) = detect() {
        start(app);
        return Ok(engine.label().to_string());
    }

    run_bun_installer()?;

    if let Some(dir) = bun_bin_dir() {
        prepend_path(&dir);
    }

    match detect() {
        Some(engine) => {
            start(app);
            Ok(engine.label().to_string())
        }
        None => Err("Bun was installed but bunx isn't on PATH".to_string()),
    }
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
