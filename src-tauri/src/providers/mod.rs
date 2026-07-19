//! Provider-neutral contracts. Adapters own provider-specific wire formats;
//! the rest of the application consumes these discriminated domain shapes.

pub mod claude;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// Starts every enabled provider adapter. The native shell calls this registry
/// once; adapters remain responsible for their own worker topology.
pub fn start_enabled(app: AppHandle) {
    claude::start(app);
}

#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum ProviderId {
    #[default]
    Claude,
    Codex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceQuality {
    Official,
    Estimated,
    Stale,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderComponent {
    Engine,
    Sensor,
    History,
    Transcript,
    Permissions,
    AppServer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HealthStatus {
    Connected,
    Degraded,
    Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderMarker {
    pub provider: ProviderId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderHealth {
    pub provider: ProviderId,
    pub component: ProviderComponent,
    pub status: HealthStatus,
    pub observed_at_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageSnapshot {
    pub provider: ProviderId,
    pub scope: String,
    pub observed_at_ms: i64,
    pub source_quality: SourceQuality,
    pub total_tokens: u64,
    pub cost_usd: Option<f64>,
    pub started_at_ms: Option<i64>,
    #[serde(default)]
    pub model_breakdown: Vec<ModelUsage>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelUsage {
    pub model_id: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnRate {
    pub provider: ProviderId,
    pub session_or_thread_id: String,
    pub observed_at_ms: i64,
    pub output_tokens: u64,
    pub elapsed_ms: i64,
    pub tokens_per_second: f64,
    pub partial: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitSnapshot {
    pub provider: ProviderId,
    pub observed_at_ms: i64,
    pub source_quality: SourceQuality,
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindow {
    pub used_percent: f64,
    pub window_duration_minutes: Option<u64>,
    pub resets_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelActivity {
    pub provider: ProviderId,
    pub model_id: String,
    pub session_or_thread_id: String,
    pub observed_at_ms: i64,
    pub sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub provider: ProviderId,
    pub request_id: String,
    pub session_id: Option<String>,
    pub prompt_id: Option<String>,
    pub turn_id: Option<String>,
    pub thread_id: Option<String>,
    pub item_id: Option<String>,
    pub tool_name: String,
    pub summary: String,
    pub cwd: String,
    pub received_at_ms: i64,
    pub expires_at_ms: i64,
    #[serde(default)]
    pub native_permission_suggestions: Vec<serde_json::Value>,
}

pub type ProviderHealthState = Arc<Mutex<HashMap<(ProviderId, ProviderComponent), ProviderHealth>>>;

pub fn new_health_state() -> ProviderHealthState {
    Arc::new(Mutex::new(HashMap::new()))
}

fn record_health(state: &ProviderHealthState, mut health: ProviderHealth) -> ProviderHealth {
    let mut registry = state.lock().unwrap();
    if let Some(current) = registry.get(&(health.provider, health.component)) {
        health.observed_at_ms = health.observed_at_ms.max(current.observed_at_ms + 1);
    }
    registry.insert((health.provider, health.component), health.clone());
    health
}

fn health_snapshot(state: &ProviderHealthState) -> Vec<ProviderHealth> {
    let mut snapshot: Vec<_> = state.lock().unwrap().values().cloned().collect();
    snapshot.sort_by_key(|health| (health.provider, health.component));
    snapshot
}

#[tauri::command]
pub fn provider_health_snapshot(app: AppHandle) -> Vec<ProviderHealth> {
    let state = app.state::<ProviderHealthState>();
    health_snapshot(&state)
}

pub fn now_epoch_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or_default()
}

pub fn emit_health(
    app: &AppHandle,
    provider: ProviderId,
    component: ProviderComponent,
    status: HealthStatus,
    detail: Option<String>,
) {
    let health = ProviderHealth {
        provider,
        component,
        status,
        observed_at_ms: now_epoch_ms(),
        detail,
    };
    let state = app.state::<ProviderHealthState>();
    let health = record_health(&state, health);
    let _ = app.emit("provider-health", health);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provider_id_serializes_as_stable_lowercase_discriminator() {
        assert_eq!(serde_json::to_value(ProviderId::Claude).unwrap(), "claude");
        assert_eq!(serde_json::to_value(ProviderId::Codex).unwrap(), "codex");
    }

    #[test]
    fn provider_health_keeps_component_failure_local() {
        let value = serde_json::to_value(ProviderHealth {
            provider: ProviderId::Codex,
            component: ProviderComponent::AppServer,
            status: HealthStatus::Degraded,
            observed_at_ms: 42,
            detail: Some("capability unavailable".into()),
        })
        .unwrap();
        assert_eq!(value["provider"], "codex");
        assert_eq!(value["component"], "app-server");
        assert_eq!(value["status"], "degraded");
        assert_eq!(value["observedAtMs"], 42);
    }

    #[test]
    fn normalized_rate_limit_preserves_source_quality() {
        let snapshot = RateLimitSnapshot {
            provider: ProviderId::Claude,
            observed_at_ms: 42,
            source_quality: SourceQuality::Official,
            primary: Some(RateLimitWindow {
                used_percent: 25.0,
                window_duration_minutes: Some(300),
                resets_at_ms: Some(1000),
            }),
            secondary: None,
        };
        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["sourceQuality"], "official");
        assert_eq!(value["primary"]["windowDurationMinutes"], 300);
        assert!(value["secondary"].is_null());
    }

    #[test]
    fn health_emitted_before_frontend_subscription_remains_in_snapshot() {
        let state = new_health_state();
        record_health(
            &state,
            ProviderHealth {
                provider: ProviderId::Claude,
                component: ProviderComponent::Engine,
                status: HealthStatus::Connected,
                observed_at_ms: 10,
                detail: None,
            },
        );
        record_health(
            &state,
            ProviderHealth {
                provider: ProviderId::Claude,
                component: ProviderComponent::Engine,
                status: HealthStatus::Degraded,
                observed_at_ms: 5,
                detail: Some("poll failed".into()),
            },
        );

        let snapshot = health_snapshot(&state);
        assert_eq!(snapshot.len(), 1);
        assert_eq!(snapshot[0].status, HealthStatus::Degraded);
        assert_eq!(snapshot[0].observed_at_ms, 11);
    }
}
