//! Process-wide guard for environment variable reads.
//!
//! `PATH` is no longer mutated anywhere in this crate post-startup (see
//! `path_state.rs`: the resolved PATH lives in app-managed state and is
//! applied per-`Command::env(...)` instead of via `std::env::set_var`). What
//! remains here are legitimate reads of `HOME`/`CLAUDE_CONFIG_DIR`/PATH-fallback
//! from the `engine`/`burn`/`sensor` poll threads. This lock centralizes
//! those reads under one call site — it does NOT protect against `getenv`/
//! `setenv` made by Tauri/objc2/libc internals outside this module (they
//! don't take it), so it's a namespacing convenience, not a soundness
//! guarantee against every possible env race in the process.

use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Reads an environment variable under the process-wide env lock.
pub(crate) fn var_os(key: &str) -> Option<std::ffi::OsString> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::var_os(key)
}
