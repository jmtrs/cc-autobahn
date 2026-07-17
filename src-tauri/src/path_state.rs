//! Process-wide resolved `PATH` for engine subprocess spawns
//! (`ccusage`/`npx`/`bunx`, Bun installer). Computed once at startup
//! (`pathfix::apply`) and updated after a successful Bun install
//! (`engine::install::install_bun`), instead of mutating the real process
//! environment: `std::env::set_var` is backed by POSIX `setenv`,
//! unsynchronized against `getenv` calls made by Tauri/objc2/libc internals
//! outside this crate's control (`env_lock` only serializes calls that go
//! through it). Storing the resolved PATH in app-managed state and passing it
//! per-`Command::env(...)` sidesteps the race entirely instead of trying to
//! win it.

use std::sync::{Arc, Mutex};

pub type PathState = Arc<Mutex<Option<String>>>;

pub(crate) fn get(state: &PathState) -> Option<String> {
    crate::window::lock(state).clone()
}

pub(crate) fn set(state: &PathState, path: String) {
    *crate::window::lock(state) = Some(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_get_set() {
        let state: PathState = Arc::new(Mutex::new(None));
        assert_eq!(get(&state), None);
        set(&state, "/opt/homebrew/bin:/usr/bin".to_string());
        assert_eq!(get(&state).as_deref(), Some("/opt/homebrew/bin:/usr/bin"));
    }
}
