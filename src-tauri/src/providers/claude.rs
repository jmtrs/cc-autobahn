//! Claude provider adapter.
//!
//! Existing Claude sources keep their mature parsing and retry behavior in
//! their concern modules. This adapter owns their lifecycle so the Tauri shell
//! does not need provider-specific startup knowledge.

use tauri::AppHandle;

use super::{ProviderComponent, ProviderId};

pub const ID: ProviderId = ProviderId::Claude;

pub const COMPONENTS: [ProviderComponent; 3] = [
    ProviderComponent::Engine,
    ProviderComponent::Transcript,
    ProviderComponent::Sensor,
];

pub fn start(app: AppHandle) {
    crate::engine::start(app.clone());
    crate::burn::start(app.clone());
    crate::sensor::start(app);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_declares_each_worker_component_once() {
        assert_eq!(ID, ProviderId::Claude);
        assert_eq!(COMPONENTS.len(), 3);

        let mut components = COMPONENTS.to_vec();
        components.sort();
        components.dedup();
        assert_eq!(components.len(), COMPONENTS.len());
        assert!(components.contains(&ProviderComponent::Engine));
        assert!(components.contains(&ProviderComponent::Transcript));
        assert!(components.contains(&ProviderComponent::Sensor));
    }
}
