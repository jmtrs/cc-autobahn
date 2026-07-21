//! Provider-neutral contracts. Adapters own provider-specific wire formats;
//! the rest of the application consumes these discriminated domain shapes.

pub mod claude;
pub mod codex;
mod diagnostics;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager};

/// Starts every enabled provider adapter. The native shell calls this registry
/// once; adapters remain responsible for their own worker topology.
pub fn start_enabled(app: AppHandle) {
    claude::start(app.clone());
    codex::start(app);
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
    Local,
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
    pub source_quality: SourceQuality,
    pub session_or_thread_id: String,
    pub session_started_at_ms: Option<i64>,
    pub observed_at_ms: i64,
    pub output_tokens: u64,
    pub elapsed_ms: i64,
    pub tokens_per_second: f64,
    pub partial: bool,
    /// Current turn's context window fill, 0-100 (`last_token_usage.total_tokens /
    /// model_context_window`). `None` when either figure was missing from the payload.
    pub context_used_pct: Option<f64>,
    /// Current turn's prompt-cache hit rate, 0-100 (`cached_input_tokens / input_tokens`
    /// from `last_token_usage` — `cached_input_tokens` is a subset of `input_tokens` in
    /// Codex's schema, not additional to it).
    pub cache_hit_pct: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitSnapshot {
    pub provider: ProviderId,
    pub observed_at_ms: i64,
    pub source_quality: SourceQuality,
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub buckets: Vec<RateLimitBucket>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitWindow {
    pub used_percent: f64,
    pub window_duration_minutes: Option<u64>,
    pub resets_at_ms: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RateLimitBucket {
    pub limit_id: Option<String>,
    pub limit_name: Option<String>,
    pub plan_type: Option<String>,
    pub primary: Option<RateLimitWindow>,
    pub secondary: Option<RateLimitWindow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountUsageSnapshot {
    pub provider: ProviderId,
    pub observed_at_ms: i64,
    pub source_quality: SourceQuality,
    pub lifetime_tokens: Option<u64>,
    pub peak_daily_tokens: Option<u64>,
    pub longest_running_turn_seconds: Option<u64>,
    pub current_streak_days: Option<u64>,
    pub longest_streak_days: Option<u64>,
    #[serde(default)]
    pub daily_usage: Vec<AccountDailyUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccountDailyUsage {
    pub start_date: String,
    pub tokens: u64,
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityDiagnostic {
    pub id: String,
    pub status: String,
    pub source: String,
    pub quality: String,
    pub fallback: Option<String>,
    pub reason: Option<String>,
    pub remediation: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderDiagnostics {
    pub provider: ProviderId,
    pub surface: String,
    pub runtime_executable: Option<String>,
    pub runtime_version: Option<String>,
    pub related_runtimes: Vec<RuntimeDiagnostic>,
    pub compatibility: String,
    pub capabilities: Vec<CapabilityDiagnostic>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RuntimeDiagnostic {
    pub surface: String,
    pub product_version: Option<String>,
    pub runtime_executable: Option<String>,
    pub runtime_version: Option<String>,
}

pub type ProviderHealthState = Arc<Mutex<HashMap<(ProviderId, ProviderComponent), ProviderHealth>>>;
pub type ProviderActivityState = Arc<Mutex<HashMap<ProviderId, ModelActivity>>>;

pub fn new_health_state() -> ProviderHealthState {
    Arc::new(Mutex::new(HashMap::new()))
}

pub fn new_activity_state() -> ProviderActivityState {
    Arc::new(Mutex::new(HashMap::new()))
}

fn record_activity(
    state: &ProviderActivityState,
    activity: ModelActivity,
) -> Option<ModelActivity> {
    let mut activities = state.lock().unwrap();
    let is_newer = activities.get(&activity.provider).is_none_or(|current| {
        if activity.observed_at_ms != current.observed_at_ms {
            return activity.observed_at_ms > current.observed_at_ms;
        }
        if activity.session_or_thread_id == current.session_or_thread_id {
            return activity.sequence > current.sequence;
        }
        (&activity.session_or_thread_id, &activity.model_id)
            > (&current.session_or_thread_id, &current.model_id)
    });
    if !is_newer {
        return None;
    }
    activities.insert(activity.provider, activity.clone());
    Some(activity)
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

#[tauri::command]
pub fn provider_activity_snapshot(app: AppHandle) -> Vec<ModelActivity> {
    let mut snapshot: Vec<_> = app
        .state::<ProviderActivityState>()
        .lock()
        .unwrap()
        .values()
        .cloned()
        .collect();
    snapshot.sort_by_key(|activity| (activity.observed_at_ms, activity.sequence));
    snapshot
}

/// `related_codex_runtimes()` shells out (`plutil`, `codex --version`) with
/// up to a 2s timeout per probe — a plain sync command would run that on the
/// same thread that pumps the webview's event loop, so a pending "loading"
/// repaint never gets a chance to flush before the whole snapshot is ready
/// (same class of bug as `install_bun`, D36). `spawn_blocking` moves it off
/// that thread, same fix shape as `history_daily`/`history_sessions`.
#[tauri::command]
pub async fn provider_diagnostics_snapshot(app: AppHandle) -> Vec<ProviderDiagnostics> {
    let health = app
        .state::<ProviderHealthState>()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let codex = app
        .state::<codex::app_server::AccountSensorState>()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone();
    let activity = app
        .try_state::<crate::permission::PermissionActivityState>()
        .and_then(|state| {
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .get(&ProviderId::Codex)
                .cloned()
        });
    let hook_active = crate::permission::codex_install::activity_matches_probe(
        codex.permission_hook.as_ref(),
        activity.as_ref(),
    );
    tauri::async_runtime::spawn_blocking(move || {
        diagnostics::build_provider_diagnostics(
            &health,
            codex,
            hook_active,
            diagnostics::related_codex_runtimes(),
        )
    })
    .await
    .unwrap_or_default()
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

pub fn emit_model_activity(app: &AppHandle, activity: ModelActivity) {
    let state = app.state::<ProviderActivityState>();
    if let Some(activity) = record_activity(&state, activity) {
        let _ = app.emit("model-activity", activity);
    }
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
            buckets: vec![],
        };
        let value = serde_json::to_value(snapshot).unwrap();
        assert_eq!(value["sourceQuality"], "official");
        assert_eq!(value["primary"]["windowDurationMinutes"], 300);
        assert!(value["secondary"].is_null());
    }

    #[test]
    fn local_turn_rate_serializes_provenance() {
        let value = serde_json::to_value(TurnRate {
            provider: ProviderId::Codex,
            source_quality: SourceQuality::Local,
            session_or_thread_id: "thread-1".into(),
            session_started_at_ms: Some(1),
            observed_at_ms: 42,
            output_tokens: 10,
            elapsed_ms: 1_000,
            tokens_per_second: 10.0,
            partial: false,
            context_used_pct: None,
            cache_hit_pct: None,
        })
        .unwrap();
        assert_eq!(value["sourceQuality"], "local");
        assert_eq!(value["sessionStartedAtMs"], 1);
    }

    #[test]
    fn related_runtime_serializes_as_a_distinct_surface() {
        let value = serde_json::to_value(RuntimeDiagnostic {
            surface: "ChatGPT desktop".into(),
            product_version: Some("26.715.31925".into()),
            runtime_executable: Some("/Applications/ChatGPT.app/Contents/Resources/codex".into()),
            runtime_version: Some("codex-cli 0.145.0-alpha.18".into()),
        })
        .unwrap();
        assert_eq!(value["surface"], "ChatGPT desktop");
        assert_eq!(value["productVersion"], "26.715.31925");
        assert_eq!(value["runtimeVersion"], "codex-cli 0.145.0-alpha.18");
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

    #[test]
    fn activity_registry_rejects_delayed_provider_events() {
        let state = new_activity_state();
        let newer = ModelActivity {
            provider: ProviderId::Codex,
            model_id: "gpt-new".into(),
            session_or_thread_id: "thread-new".into(),
            observed_at_ms: 200,
            sequence: 1,
        };
        assert_eq!(record_activity(&state, newer.clone()), Some(newer.clone()));
        let delayed = ModelActivity {
            provider: ProviderId::Codex,
            model_id: "gpt-old".into(),
            session_or_thread_id: "thread-old".into(),
            observed_at_ms: 199,
            sequence: 99,
        };
        assert_eq!(record_activity(&state, delayed), None);
        assert_eq!(state.lock().unwrap().get(&ProviderId::Codex), Some(&newer));
    }

    #[test]
    fn activity_registry_uses_a_stable_cross_session_tie_break() {
        let state = new_activity_state();
        let root = ModelActivity {
            provider: ProviderId::Codex,
            model_id: "gpt-root".into(),
            session_or_thread_id: "thread-z".into(),
            observed_at_ms: 200,
            sequence: 1,
        };
        let subagent = ModelActivity {
            provider: ProviderId::Codex,
            model_id: "gpt-subagent".into(),
            session_or_thread_id: "thread-a".into(),
            observed_at_ms: 200,
            sequence: 99,
        };
        assert_eq!(
            record_activity(&state, subagent),
            Some(ModelActivity {
                provider: ProviderId::Codex,
                model_id: "gpt-subagent".into(),
                session_or_thread_id: "thread-a".into(),
                observed_at_ms: 200,
                sequence: 99,
            })
        );
        assert_eq!(record_activity(&state, root.clone()), Some(root.clone()));
        assert_eq!(state.lock().unwrap().get(&ProviderId::Codex), Some(&root));
    }
}
