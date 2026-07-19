//! On-demand provider-scoped `ccusage` history reports.
//!
//! Daily data feeds History and today's model list. Session data is exposed as
//! a normalized command for provider consumers without leaking local paths.
//! Both stay outside the continuous engine loop: a report is spawned only
//! when requested and the frontend owns the short cache.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use tauri::Manager;

use crate::providers::{HealthStatus, ProviderComponent, ProviderId, SourceQuality};

const HISTORY_DAYS: i64 = 30;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyEntry {
    pub provider: ProviderId,
    pub source_quality: SourceQuality,
    pub date: String,
    pub total_cost: f64,
    pub total_tokens: u64,
    pub model_breakdowns: Vec<ModelBreakdown>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdown {
    pub model_name: String,
    pub cost: Option<f64>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub reasoning_output_tokens: u64,
    pub cache_creation_tokens: u64,
    pub cache_read_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionEntry {
    pub provider: ProviderId,
    pub source_quality: SourceQuality,
    pub session_or_thread_id: String,
    pub first_activity: Option<String>,
    pub last_activity: String,
    pub total_cost: f64,
    pub total_tokens: u64,
    pub model_breakdowns: Vec<ModelBreakdown>,
}

#[derive(Debug, Deserialize)]
struct ClaudeDailyEnvelope {
    #[serde(default)]
    daily: Vec<ClaudeDailyEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeDailyEntry {
    date: String,
    #[serde(default)]
    total_cost: f64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    model_breakdowns: Vec<ClaudeModelBreakdown>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeSessionEnvelope {
    #[serde(default)]
    sessions: Vec<ClaudeSessionEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeSessionEntry {
    session_id: String,
    first_activity: Option<String>,
    last_activity: String,
    #[serde(default)]
    total_cost: f64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    model_breakdowns: Vec<ClaudeModelBreakdown>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeModelBreakdown {
    model_name: String,
    #[serde(default)]
    cost: f64,
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
}

#[derive(Debug, Deserialize)]
struct CodexDailyEnvelope {
    #[serde(default)]
    daily: Vec<CodexDailyEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexDailyEntry {
    date: String,
    #[serde(default, rename = "costUSD")]
    cost_usd: f64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    models: BTreeMap<String, CodexModelUsage>,
}

#[derive(Debug, Deserialize)]
struct CodexSessionEnvelope {
    #[serde(default)]
    sessions: Vec<CodexSessionEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexSessionEntry {
    session_id: String,
    last_activity: String,
    #[serde(default, rename = "costUSD")]
    cost_usd: f64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    models: BTreeMap<String, CodexModelUsage>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexModelUsage {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    reasoning_output_tokens: u64,
    #[serde(default)]
    cache_creation_tokens: u64,
    #[serde(default)]
    cache_read_tokens: u64,
}

#[tauri::command]
pub async fn history_daily(
    app: tauri::AppHandle,
    provider: ProviderId,
) -> Result<Vec<DailyEntry>, String> {
    let path = crate::path_state::get(&app.state::<crate::path_state::PathState>());
    let result =
        tauri::async_runtime::spawn_blocking(move || history_daily_blocking(path, provider))
            .await
            .map_err(|error| format!("history_daily task panicked: {error}"))?;
    emit_report_health(&app, provider, &result);
    result
}

#[tauri::command]
pub async fn history_sessions(
    app: tauri::AppHandle,
    provider: ProviderId,
) -> Result<Vec<SessionEntry>, String> {
    let path = crate::path_state::get(&app.state::<crate::path_state::PathState>());
    let result =
        tauri::async_runtime::spawn_blocking(move || history_sessions_blocking(path, provider))
            .await
            .map_err(|error| format!("history_sessions task panicked: {error}"))?;
    emit_report_health(&app, provider, &result);
    result
}

fn emit_report_health<T>(app: &tauri::AppHandle, provider: ProviderId, result: &Result<T, String>) {
    let (status, detail) = match result {
        Ok(_) => (HealthStatus::Connected, None),
        Err(error) if error == "no engine available" => {
            (HealthStatus::Unavailable, Some(error.clone()))
        }
        Err(error) => (HealthStatus::Degraded, Some(error.clone())),
    };
    crate::providers::emit_health(app, provider, ProviderComponent::History, status, detail);
}

fn history_daily_blocking(
    path: Option<String>,
    provider: ProviderId,
) -> Result<Vec<DailyEntry>, String> {
    let stdout = run_report(path, provider, "daily")?;
    parse_daily(provider, &stdout)
}

fn history_sessions_blocking(
    path: Option<String>,
    provider: ProviderId,
) -> Result<Vec<SessionEntry>, String> {
    let stdout = run_report(path, provider, "session")?;
    parse_sessions(provider, &stdout)
}

fn run_report(path: Option<String>, provider: ProviderId, report: &str) -> Result<Vec<u8>, String> {
    let engine = super::detect(path.as_deref()).ok_or("no engine available")?;
    let since = since_date(HISTORY_DAYS);
    let mut command = engine.base_command(path.as_deref());
    match provider {
        ProviderId::Claude => {
            command.args(["claude", report, "--json", "--since", &since]);
        }
        ProviderId::Codex => {
            command.args([
                "codex", report, "--json", "--since", &since, "--speed", "auto",
            ]);
        }
    }
    let output = command
        .output()
        .map_err(|error| format!("could not launch {}: {error}", engine.label()))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ccusage exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }
    Ok(output.stdout)
}

fn parse_daily(provider: ProviderId, bytes: &[u8]) -> Result<Vec<DailyEntry>, String> {
    match provider {
        ProviderId::Claude => {
            let envelope: ClaudeDailyEnvelope = parse_json(bytes)?;
            Ok(envelope
                .daily
                .into_iter()
                .map(normalize_claude_daily)
                .collect())
        }
        ProviderId::Codex => {
            let envelope: CodexDailyEnvelope = parse_json(bytes)?;
            Ok(envelope
                .daily
                .into_iter()
                .map(normalize_codex_daily)
                .collect())
        }
    }
}

fn parse_sessions(provider: ProviderId, bytes: &[u8]) -> Result<Vec<SessionEntry>, String> {
    match provider {
        ProviderId::Claude => {
            let envelope: ClaudeSessionEnvelope = parse_json(bytes)?;
            Ok(envelope
                .sessions
                .into_iter()
                .map(normalize_claude_session)
                .collect())
        }
        ProviderId::Codex => {
            let envelope: CodexSessionEnvelope = parse_json(bytes)?;
            Ok(envelope
                .sessions
                .into_iter()
                .map(normalize_codex_session)
                .collect())
        }
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|error| format!("unparseable ccusage JSON: {error}"))
}

fn normalize_claude_daily(entry: ClaudeDailyEntry) -> DailyEntry {
    DailyEntry {
        provider: ProviderId::Claude,
        source_quality: SourceQuality::Estimated,
        date: entry.date,
        total_cost: entry.total_cost,
        total_tokens: entry.total_tokens,
        model_breakdowns: entry
            .model_breakdowns
            .into_iter()
            .map(normalize_claude_model)
            .collect(),
    }
}

fn normalize_codex_daily(entry: CodexDailyEntry) -> DailyEntry {
    DailyEntry {
        provider: ProviderId::Codex,
        source_quality: SourceQuality::Estimated,
        date: entry.date,
        total_cost: entry.cost_usd,
        total_tokens: entry.total_tokens,
        model_breakdowns: normalize_codex_models(entry.models, entry.cost_usd),
    }
}

fn normalize_claude_session(entry: ClaudeSessionEntry) -> SessionEntry {
    SessionEntry {
        provider: ProviderId::Claude,
        source_quality: SourceQuality::Estimated,
        session_or_thread_id: entry.session_id,
        first_activity: entry.first_activity,
        last_activity: entry.last_activity,
        total_cost: entry.total_cost,
        total_tokens: entry.total_tokens,
        model_breakdowns: entry
            .model_breakdowns
            .into_iter()
            .map(normalize_claude_model)
            .collect(),
    }
}

fn normalize_codex_session(entry: CodexSessionEntry) -> SessionEntry {
    SessionEntry {
        provider: ProviderId::Codex,
        source_quality: SourceQuality::Estimated,
        session_or_thread_id: entry.session_id,
        first_activity: None,
        last_activity: entry.last_activity,
        total_cost: entry.cost_usd,
        total_tokens: entry.total_tokens,
        model_breakdowns: normalize_codex_models(entry.models, entry.cost_usd),
    }
}

fn normalize_claude_model(entry: ClaudeModelBreakdown) -> ModelBreakdown {
    ModelBreakdown {
        model_name: entry.model_name,
        cost: Some(entry.cost),
        input_tokens: entry.input_tokens,
        output_tokens: entry.output_tokens,
        reasoning_output_tokens: 0,
        cache_creation_tokens: entry.cache_creation_tokens,
        cache_read_tokens: entry.cache_read_tokens,
    }
}

fn normalize_codex_models(
    models: BTreeMap<String, CodexModelUsage>,
    aggregate_cost: f64,
) -> Vec<ModelBreakdown> {
    let sole_model = models.len() == 1;
    models
        .into_iter()
        .map(|(model_name, usage)| ModelBreakdown {
            model_name,
            cost: sole_model.then_some(aggregate_cost),
            input_tokens: usage.input_tokens,
            output_tokens: usage.output_tokens,
            reasoning_output_tokens: usage.reasoning_output_tokens,
            cache_creation_tokens: usage.cache_creation_tokens,
            cache_read_tokens: usage.cache_read_tokens,
        })
        .collect()
}

fn since_date(days_ago: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis() as i64)
        .unwrap_or(0);
    let epoch_days = now_ms / 86_400_000 - days_ago;
    let (year, month, day) = civil_from_days(epoch_days);
    format!("{year:04}{month:02}{day:02}")
}

fn civil_from_days(days: i64) -> (i64, u64, u64) {
    let days = days + 719_468;
    let era = if days >= 0 { days } else { days - 146_096 } / 146_097;
    let day_of_era = (days - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146_096) / 365;
    let year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (year + i64::from(month <= 2), month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE_DAILY: &str = r#"{
      "daily": [{
        "date": "2026-07-17", "totalCost": 2.12, "totalTokens": 4568583,
        "modelBreakdowns": [{
          "cacheCreationTokens": 189093, "cacheReadTokens": 4316477,
          "cost": 2.12, "inputTokens": 15336,
          "modelName": "claude-sonnet-5", "outputTokens": 47677
        }]
      }]
    }"#;

    const CODEX_DAILY_ONE_MODEL: &str = r#"{
      "daily": [{
        "date": "2026-07-19", "costUSD": 1.25, "totalTokens": 1200,
        "models": {"gpt-5.6-sol": {
          "inputTokens": 800, "outputTokens": 250,
          "reasoningOutputTokens": 50, "cacheCreationTokens": 0,
          "cacheReadTokens": 100, "totalTokens": 1200, "isFallback": false
        }}
      }]
    }"#;

    const CODEX_DAILY_MULTI_MODEL: &str = r#"{
      "daily": [{
        "date": "2026-07-19", "costUSD": 2.5, "totalTokens": 3000,
        "models": {
          "gpt-5.6-sol": {"inputTokens": 1000, "outputTokens": 200},
          "gpt-5.6-terra": {"inputTokens": 1500, "outputTokens": 300}
        }
      }]
    }"#;

    const CODEX_SESSIONS: &str = r#"{
      "sessions": [{
        "sessionId": "thread-1", "sessionFile": "/private/rollout.jsonl",
        "directory": "/private/project", "lastActivity": "2026-07-19T12:00:00Z",
        "costUSD": 0.75, "totalTokens": 600,
        "models": {"gpt-5.6-sol": {"inputTokens": 400, "outputTokens": 200}}
      }]
    }"#;

    #[test]
    fn parses_and_normalizes_claude_daily() {
        let days = parse_daily(ProviderId::Claude, CLAUDE_DAILY.as_bytes()).unwrap();
        assert_eq!(days.len(), 1);
        assert_eq!(days[0].provider, ProviderId::Claude);
        assert_eq!(days[0].model_breakdowns[0].cost, Some(2.12));
        assert_eq!(days[0].model_breakdowns[0].cache_read_tokens, 4_316_477);
    }

    #[test]
    fn codex_single_model_can_receive_aggregate_cost() {
        let days = parse_daily(ProviderId::Codex, CODEX_DAILY_ONE_MODEL.as_bytes()).unwrap();
        let day = &days[0];
        assert_eq!(day.provider, ProviderId::Codex);
        assert_eq!(day.source_quality, SourceQuality::Estimated);
        assert_eq!(day.model_breakdowns[0].cost, Some(1.25));
        assert_eq!(day.model_breakdowns[0].reasoning_output_tokens, 50);
    }

    #[test]
    fn codex_multi_model_cost_is_not_invented() {
        let days = parse_daily(ProviderId::Codex, CODEX_DAILY_MULTI_MODEL.as_bytes()).unwrap();
        assert_eq!(days[0].model_breakdowns.len(), 2);
        assert!(days[0]
            .model_breakdowns
            .iter()
            .all(|model| model.cost.is_none()));
        assert_eq!(days[0].total_cost, 2.5);
    }

    #[test]
    fn session_normalization_drops_local_paths() {
        let sessions = parse_sessions(ProviderId::Codex, CODEX_SESSIONS.as_bytes()).unwrap();
        let value = serde_json::to_value(&sessions[0]).unwrap();
        assert_eq!(value["sessionOrThreadId"], "thread-1");
        assert!(value.get("sessionFile").is_none());
        assert!(value.get("directory").is_none());
    }

    #[test]
    fn provider_is_assigned_by_adapter_not_external_json() {
        let spoofed = CODEX_DAILY_ONE_MODEL.replace(
            "\"date\": \"2026-07-19\"",
            "\"provider\": \"claude\", \"date\": \"2026-07-19\"",
        );
        let days = parse_daily(ProviderId::Codex, spoofed.as_bytes()).unwrap();
        assert_eq!(days[0].provider, ProviderId::Codex);
    }

    #[test]
    fn malformed_or_empty_envelopes_are_tolerated_deliberately() {
        assert!(parse_daily(ProviderId::Codex, b"not-json").is_err());
        assert!(parse_daily(ProviderId::Codex, b"{}").unwrap().is_empty());
    }

    #[test]
    fn civil_from_days_epoch_origin() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    #[test]
    fn civil_from_days_known_date() {
        assert_eq!(civil_from_days(20_651), (2026, 7, 17));
    }

    #[test]
    fn civil_from_days_30_days_earlier() {
        assert_eq!(civil_from_days(20_651 - 30), (2026, 6, 17));
    }
}
