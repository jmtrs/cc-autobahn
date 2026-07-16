//! Process-wide guard for environment variable access.
//!
//! `std::env::set_var`/`var_os` are backed by POSIX `setenv`/`getenv`, which
//! are not thread-safe against each other: mutating the environment on one
//! thread while another reads it is a data race (this is why Rust 2024 marks
//! `set_var` `unsafe`). `engine::install::prepend_path` mutates `PATH` once,
//! from the Tauri command-handler thread, while the `engine`/`burn`/`sensor`
//! poll threads keep reading `PATH`/`HOME`/`CLAUDE_CONFIG_DIR` in their loops.
//! Every env read/write in the crate takes this lock first to serialize
//! access instead of relying on the mutation window being "unlikely to hit".

use std::sync::Mutex;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// Reads an environment variable under the process-wide env lock.
pub(crate) fn var_os(key: &str) -> Option<std::ffi::OsString> {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::var_os(key)
}

/// Sets an environment variable under the process-wide env lock.
pub(crate) fn set_var(key: &str, value: impl AsRef<std::ffi::OsStr>) {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    std::env::set_var(key, value);
}
