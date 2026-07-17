//! On-demand `ccusage claude daily --json` fetch for the History page (Page 1)
//! and today's per-model cost split (Page 2). Deliberately NOT part of the
//! continuous `engine::start` poll loop (D13): daily totals barely move
//! within a day, so this is fetched only when the frontend opens that page,
//! not on a background timer — same "cadence matches the data" principle,
//! taken to its on-demand extreme.
//!
//! Scoped to `claude` (`ccusage claude daily`, not the top-level `ccusage
//! daily`): the top-level command mixes in every agent ccusage detects on
//! the machine (Codex, Gemini, etc.) if the user has those CLIs installed.

use serde::{Deserialize, Serialize};

/// How many days back the History page shows (D-review: a month of bars is
/// enough for a trip-computer sparkline; more would need pagination, which
/// isn't worth it yet).
const HISTORY_DAYS: i64 = 30;

#[derive(Debug, Deserialize)]
struct DailyEnvelope {
    #[serde(default)]
    daily: Vec<DailyEntry>,
}

/// One day of usage. Forwarded as-is to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DailyEntry {
    pub date: String,
    #[serde(default)]
    pub total_cost: f64,
    #[serde(default)]
    pub total_tokens: u64,
    #[serde(default)]
    pub model_breakdowns: Vec<ModelBreakdown>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelBreakdown {
    pub model_name: String,
    #[serde(default)]
    pub cost: f64,
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default)]
    pub cache_creation_tokens: u64,
    #[serde(default)]
    pub cache_read_tokens: u64,
}

/// `#[tauri::command]` Last `HISTORY_DAYS` days of Claude usage. `Err` with a
/// readable message if no engine is available or ccusage/parsing fails —
/// same contract as `engine::blocks::poll_once`.
#[tauri::command]
pub fn history_daily() -> Result<Vec<DailyEntry>, String> {
    let engine = super::detect().ok_or("no engine available")?;
    let since = since_date(HISTORY_DAYS);

    let output = engine
        .base_command()
        .args(["claude", "daily", "--json", "--since", &since])
        .output()
        .map_err(|e| format!("could not launch {}: {e}", engine.label()))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "ccusage exited with {}: {}",
            output.status,
            stderr.trim()
        ));
    }

    let envelope: DailyEnvelope = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("unparseable ccusage JSON: {e}"))?;
    Ok(envelope.daily)
}

/// `YYYYMMDD` for `days_ago` days before today (UTC). No `chrono` (D10:
/// zero new deps) — same Howard Hinnant civil-calendar algorithm as
/// `burn::zulu::days_from_civil`, run in reverse.
fn since_date(days_ago: i64) -> String {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);
    let epoch_days = now_ms / 86_400_000 - days_ago;
    let (y, m, d) = civil_from_days(epoch_days);
    format!("{y:04}{m:02}{d:02}")
}

/// Inverse of `days_from_civil`: epoch-days since 1970-01-01 → (year, month, day).
/// Proleptic Gregorian, valid for the recent dates this module deals with.
fn civil_from_days(z: i64) -> (i64, u64, u64) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    (y + i64::from(m <= 2), m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_epoch_origin() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }

    /// 2026-07-17 is epoch-day 20651 (cross-checked with Python's `datetime`).
    #[test]
    fn civil_from_days_known_date() {
        assert_eq!(civil_from_days(20_651), (2026, 7, 17));
    }

    /// 2026-07-17 minus 30 days = 2026-06-17, epoch-day 20621.
    #[test]
    fn civil_from_days_30_days_earlier() {
        assert_eq!(civil_from_days(20_651 - 30), (2026, 6, 17));
    }

    /// Real output of `ccusage v20 claude daily --json` (captured 2026-07-17).
    const REAL_SAMPLE: &str = r#"{
      "daily": [
        {
          "cacheCreationTokens": 189093,
          "cacheReadTokens": 4316477,
          "date": "2026-07-17",
          "inputTokens": 15336,
          "modelBreakdowns": [
            { "cacheCreationTokens": 189093, "cacheReadTokens": 4316477, "cost": 2.1271093999999997, "inputTokens": 15336, "modelName": "claude-sonnet-5", "outputTokens": 47677 }
          ],
          "modelsUsed": ["claude-sonnet-5"],
          "outputTokens": 47677,
          "totalCost": 2.1271093999999997,
          "totalTokens": 4568583
        }
      ],
      "totals": {
        "cacheCreationTokens": 189093, "cacheReadTokens": 4316477, "inputTokens": 15336,
        "outputTokens": 47677, "totalCost": 2.1271093999999997, "totalTokens": 4568583
      }
    }"#;

    #[test]
    fn parses_real_daily_sample() {
        let env: DailyEnvelope = serde_json::from_str(REAL_SAMPLE).expect("must parse");
        assert_eq!(env.daily.len(), 1);
        let day = &env.daily[0];
        assert_eq!(day.date, "2026-07-17");
        assert_eq!(day.model_breakdowns.len(), 1);
        let m = &day.model_breakdowns[0];
        assert_eq!(m.model_name, "claude-sonnet-5");
        assert_eq!(m.output_tokens, 47_677);
        assert_eq!(m.cache_read_tokens, 4_316_477);
    }

    /// Real output for a day with 3 models (captured 2026-07-17, `--since
    /// 20260701 --until 20260710`), including a routed non-Claude model
    /// (glm) — regression for a bug where the frontend showed "0" tokens for
    /// every model despite non-zero `cost`: confirms all 4 token fields
    /// parse for every entry in a multi-model breakdown, not just the first.
    const REAL_MULTI_MODEL_SAMPLE: &str = r#"{
      "daily": [{
        "date": "2026-07-07",
        "modelBreakdowns": [
          { "cacheCreationTokens": 1029008, "cacheReadTokens": 67842671, "cost": 20.61724870000002, "inputTokens": 64771, "modelName": "claude-sonnet-5", "outputTokens": 300155 },
          { "cacheCreationTokens": 0, "cacheReadTokens": 15200640, "cost": 5.172576199999998, "inputTokens": 508869, "modelName": "glm-5.2", "outputTokens": 115453 },
          { "cacheCreationTokens": 0, "cacheReadTokens": 879104, "cost": 0.15685383999999997, "inputTokens": 55594, "modelName": "glm-4.7", "outputTokens": 12180 }
        ],
        "modelsUsed": ["claude-sonnet-5", "glm-5.2", "glm-4.7"],
        "outputTokens": 427788, "totalCost": 25.946678740000017, "totalTokens": 86008445
      }]
    }"#;

    #[test]
    fn parses_real_multi_model_sample() {
        let env: DailyEnvelope =
            serde_json::from_str(REAL_MULTI_MODEL_SAMPLE).expect("must parse");
        let models = &env.daily[0].model_breakdowns;
        assert_eq!(models.len(), 3);
        for m in models {
            assert!(m.cost > 0.0, "{} should have non-zero cost", m.model_name);
            assert!(
                m.input_tokens + m.output_tokens + m.cache_read_tokens > 0,
                "{} should have non-zero tokens",
                m.model_name
            );
        }
        assert_eq!(models[1].model_name, "glm-5.2");
        assert_eq!(models[1].input_tokens, 508_869);
    }

    #[test]
    fn tolerates_empty() {
        let env: DailyEnvelope = serde_json::from_str("{}").expect("empty parses");
        assert!(env.daily.is_empty());
    }
}
