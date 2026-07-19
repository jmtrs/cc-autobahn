//! Codex provider adapter.
//!
//! Phase 3 starts with the local rollout source so activity from independent
//! CLI, IDE and desktop threads remains observable without an owned App Server.

pub mod app_server;
mod rollout;

use std::thread;
use std::time::Duration;

use tauri::AppHandle;

use super::{emit_health, HealthStatus, ProviderComponent, ProviderId};

pub const ID: ProviderId = ProviderId::Codex;
pub const COMPONENTS: [ProviderComponent; 2] =
    [ProviderComponent::Transcript, ProviderComponent::AppServer];

const TAIL_INTERVAL_MS: u64 = 200;
const RESCAN_SECS: u64 = 5;

pub fn start(app: AppHandle) {
    app_server::start(app.clone());
    thread::spawn(move || {
        let roots = rollout::discover_rollout_roots();
        let mut tails = rollout::TailSet::new();
        let scan_every = (RESCAN_SECS * 1000 / TAIL_INTERVAL_MS).max(1);
        let mut tick = 0u64;
        let mut last_available = None;

        loop {
            if tick.is_multiple_of(scan_every) {
                let discovered = tails.rescan(&roots, &app);
                let available = discovered > 0;
                if last_available != Some(available) {
                    emit_health(
                        &app,
                        ID,
                        ProviderComponent::Transcript,
                        if available {
                            HealthStatus::Connected
                        } else {
                            HealthStatus::Unavailable
                        },
                        (!available).then(|| "no recent Codex rollout found".to_string()),
                    );
                    last_available = Some(available);
                }
            }
            tails.pump(&app);
            tick = tick.wrapping_add(1);
            thread::sleep(Duration::from_millis(TAIL_INTERVAL_MS));
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_declares_rollout_transcript_once() {
        assert_eq!(ID, ProviderId::Codex);
        assert_eq!(COMPONENTS.len(), 2);
        assert!(COMPONENTS.contains(&ProviderComponent::Transcript));
        assert!(COMPONENTS.contains(&ProviderComponent::AppServer));
    }
}
