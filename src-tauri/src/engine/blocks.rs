//! serde model for the `ccusage blocks --active --json` JSON + the poll itself.
//! Structured against the real output (ccusage v20; captured 2026-07-16).
//! Optional/`default` fields because "gap" blocks omit several of them.

use serde::{Deserialize, Serialize};

use super::Engine;

#[derive(Debug, Deserialize)]
struct BlocksEnvelope {
    #[serde(default)]
    blocks: Vec<Block>,
}

/// A 5 h billing block. Forwarded as-is to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Block {
    id: String,
    #[serde(default)]
    is_active: bool,
    #[serde(default)]
    is_gap: bool,
    #[serde(default)]
    start_time: String,
    #[serde(default)]
    end_time: String,
    #[serde(default)]
    actual_end_time: Option<String>,
    #[serde(default)]
    cost_usd: f64,
    #[serde(default)]
    total_tokens: u64,
    #[serde(default)]
    entries: u64,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    token_counts: TokenCounts,
    #[serde(default)]
    burn_rate: Option<BurnRate>,
    #[serde(default)]
    pub(crate) projection: Option<Projection>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TokenCounts {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    cache_creation_input_tokens: u64,
    #[serde(default)]
    cache_read_input_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct BurnRate {
    #[serde(default)]
    cost_per_hour: f64,
    #[serde(default)]
    tokens_per_minute: f64,
    /// Smoothed average from ccusage. NOT our per-response `tok/s` (D8):
    /// that one is computed by the JSONL tail in Phase 2.
    #[serde(default)]
    tokens_per_minute_for_indicator: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Projection {
    #[serde(default)]
    pub(crate) remaining_minutes: u64,
    #[serde(default)]
    total_cost: f64,
    #[serde(default)]
    total_tokens: u64,
}

/// Runs ccusage once and returns the active block (if any).
/// `Err` with a readable message on any spawn / exit / parse failure.
pub(crate) fn poll_once(engine: Engine, path: Option<&str>) -> Result<Option<Block>, String> {
    let output = engine
        .base_command(path)
        .args(["blocks", "--active", "--json"])
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

    let envelope: BlocksEnvelope = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("unparseable ccusage JSON: {e}"))?;

    Ok(envelope
        .blocks
        .into_iter()
        .find(|b| b.is_active && !b.is_gap))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real output of `ccusage v20 blocks --active --json` (captured 2026-07-16).
    /// Locks the serde model's contract against the real JSON.
    const REAL_SAMPLE: &str = r#"{
      "blocks": [{
        "actualEndTime": "2026-07-16T08:54:57.757Z",
        "burnRate": { "costPerHour": 16.81, "tokensPerMinute": 313145.96, "tokensPerMinuteForIndicator": 3093.26 },
        "costUSD": 24.846,
        "endTime": "2026-07-16T12:00:00.000Z",
        "entries": 262,
        "id": "2026-07-16T07:00:00.000Z",
        "isActive": true,
        "isGap": false,
        "models": ["claude-opus-4-8"],
        "projection": { "remainingMinutes": 185, "totalCost": 76.68, "totalTokens": 85701641 },
        "startTime": "2026-07-16T07:00:00.000Z",
        "tokenCounts": { "cacheCreationInputTokens": 544396, "cacheReadInputTokens": 26950933, "inputTokens": 46557, "outputTokens": 227752 },
        "totalTokens": 27769638
      }]
    }"#;

    #[test]
    fn parses_real_active_block() {
        let env: BlocksEnvelope = serde_json::from_str(REAL_SAMPLE).expect("must parse");
        let block = env
            .blocks
            .into_iter()
            .find(|b| b.is_active && !b.is_gap)
            .expect("there's an active block");
        assert_eq!(block.total_tokens, 27_769_638);
        assert_eq!(block.token_counts.output_tokens, 227_752);
        assert_eq!(block.projection.unwrap().remaining_minutes, 185);
        assert!(block.burn_rate.unwrap().cost_per_hour > 0.0);
    }

    /// A "gap" block omits burnRate/projection: it must not break parsing.
    #[test]
    fn tolerates_gap_block_missing_fields() {
        let json = r#"{"blocks":[{"id":"x","isGap":true,"isActive":false}]}"#;
        let env: BlocksEnvelope = serde_json::from_str(json).expect("gap parses");
        let b = &env.blocks[0];
        assert!(b.is_gap);
        assert!(b.burn_rate.is_none());
        assert!(b.projection.is_none());
    }

    /// Empty JSON / no blocks: valid envelope, zero blocks.
    #[test]
    fn tolerates_empty() {
        let env: BlocksEnvelope = serde_json::from_str("{}").expect("empty parses");
        assert!(env.blocks.is_empty());
    }
}
