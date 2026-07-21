//! Pure JSON-RPC payload normalization for the Codex App Server sensor.
//!
//! Nothing here touches the sensor state, the child process, or the
//! `AppHandle`; every function is a total mapping from a wire `Value` to a
//! provider-neutral shape (or `None` when the response is incompatible). This
//! is where the App Server wire assumptions are isolated and unit-tested.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{json, Value};

use super::{CodexHookProbe, ID};
use crate::providers::{
    now_epoch_ms, AccountDailyUsage, AccountUsageSnapshot, HealthStatus, RateLimitBucket,
    RateLimitSnapshot, RateLimitWindow, SourceQuality,
};

pub(super) fn merge_rate_limit_update(current: &mut Option<Value>, incoming: &Value) {
    let response = current.get_or_insert_with(|| json!({ "rateLimits": {} }));
    let Some(object) = response.as_object_mut() else {
        return;
    };
    let incoming_limit_id = incoming.get("limitId").and_then(Value::as_str);
    let legacy = object.entry("rateLimits").or_insert_with(|| json!({}));
    let legacy_limit_id = legacy.get("limitId").and_then(Value::as_str);
    if incoming_limit_id.is_none()
        || legacy_limit_id.is_none()
        || incoming_limit_id == legacy_limit_id
    {
        merge_non_null(legacy, incoming);
    }
    let buckets = object
        .entry("rateLimitsByLimitId")
        .or_insert_with(|| json!({}));
    if !buckets.is_object() {
        *buckets = json!({});
    }
    if let Some(buckets) = buckets.as_object_mut() {
        if let Some(limit_id) = incoming_limit_id {
            merge_non_null(
                buckets.entry(limit_id).or_insert_with(|| json!({})),
                incoming,
            );
        } else if let Some(codex) = buckets.get_mut("codex") {
            merge_non_null(codex, incoming);
        } else if buckets.len() == 1 {
            if let Some(only_bucket) = buckets.values_mut().next() {
                merge_non_null(only_bucket, incoming);
            }
        }
    }
}

fn merge_non_null(current: &mut Value, incoming: &Value) {
    if incoming.is_null() {
        return;
    }
    match (current.as_object_mut(), incoming.as_object()) {
        (Some(current), Some(incoming)) => {
            for (key, value) in incoming {
                if value.is_null() {
                    continue;
                }
                merge_non_null(current.entry(key).or_insert(Value::Null), value);
            }
        }
        _ => *current = incoming.clone(),
    }
}

pub(super) fn normalize_limits(value: &Value, quality: SourceQuality) -> Option<RateLimitSnapshot> {
    let legacy = value.get("rateLimits").filter(|value| value.is_object());
    let mut raw_buckets = BTreeMap::new();
    if let Some(buckets) = value.get("rateLimitsByLimitId").and_then(Value::as_object) {
        for (id, bucket) in buckets {
            if bucket.is_object() {
                raw_buckets.insert(id.clone(), bucket);
            }
        }
    }
    if let Some(legacy) = legacy {
        if let Some(limit_id) = legacy.get("limitId").and_then(Value::as_str) {
            raw_buckets.entry(limit_id.to_string()).or_insert(legacy);
        }
    }
    let selected = raw_buckets
        .get("codex")
        .copied()
        .or_else(|| {
            legacy.filter(|bucket| bucket.get("limitId").and_then(Value::as_str) == Some("codex"))
        })
        .or(legacy)
        .or_else(|| raw_buckets.values().next().copied())?;

    let buckets = if raw_buckets.is_empty() {
        legacy.into_iter().map(normalize_bucket).collect()
    } else {
        raw_buckets
            .values()
            .map(|bucket| normalize_bucket(bucket))
            .collect()
    };
    Some(RateLimitSnapshot {
        provider: ID,
        observed_at_ms: now_epoch_ms(),
        source_quality: quality,
        primary: normalize_window(selected.get("primary")),
        secondary: normalize_window(selected.get("secondary")),
        buckets,
    })
}

fn normalize_bucket(value: &Value) -> RateLimitBucket {
    RateLimitBucket {
        limit_id: string_field(value, "limitId"),
        limit_name: string_field(value, "limitName"),
        plan_type: string_field(value, "planType"),
        primary: normalize_window(value.get("primary")),
        secondary: normalize_window(value.get("secondary")),
    }
}

fn normalize_window(value: Option<&Value>) -> Option<RateLimitWindow> {
    let value = value?.as_object()?;
    let used_percent = value.get("usedPercent")?.as_f64()?.clamp(0.0, 100.0);
    Some(RateLimitWindow {
        used_percent,
        window_duration_minutes: value.get("windowDurationMins").and_then(Value::as_u64),
        resets_at_ms: value
            .get("resetsAt")
            .and_then(Value::as_i64)
            .and_then(|seconds| seconds.checked_mul(1000)),
    })
}

pub(super) fn normalize_usage(
    value: &Value,
    quality: SourceQuality,
) -> Option<AccountUsageSnapshot> {
    let summary = value.get("summary")?.as_object()?;
    let daily_usage = value
        .get("dailyUsageBuckets")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|bucket| {
            Some(AccountDailyUsage {
                start_date: bucket.get("startDate")?.as_str()?.to_string(),
                tokens: bucket.get("tokens")?.as_u64()?,
            })
        })
        .collect();
    Some(AccountUsageSnapshot {
        provider: ID,
        observed_at_ms: now_epoch_ms(),
        source_quality: quality,
        lifetime_tokens: summary.get("lifetimeTokens").and_then(Value::as_u64),
        peak_daily_tokens: summary.get("peakDailyTokens").and_then(Value::as_u64),
        longest_running_turn_seconds: summary.get("longestRunningTurnSec").and_then(Value::as_u64),
        current_streak_days: summary.get("currentStreakDays").and_then(Value::as_u64),
        longest_streak_days: summary.get("longestStreakDays").and_then(Value::as_u64),
        daily_usage,
    })
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key).and_then(Value::as_str).map(str::to_string)
}

pub(super) fn normalize_hook_probe(
    hook: &Value,
    expected_path: Option<&Path>,
) -> Option<CodexHookProbe> {
    let command = hook.get("command")?.as_str()?;
    let expected_path = expected_path?;
    let expected_command = crate::permission::codex_install::hook_command(expected_path.parent()?);
    if command != expected_command
        || hook.get("handlerType").and_then(Value::as_str) != Some("command")
        || hook.get("source").and_then(Value::as_str) != Some("user")
        || !matches!(
            hook.get("eventName").and_then(Value::as_str),
            Some("permissionRequest" | "permission_request")
        )
    {
        return None;
    }
    let source_path = hook.get("sourcePath")?.as_str()?;
    if !same_path(Path::new(source_path), expected_path) {
        return None;
    }
    Some(CodexHookProbe {
        enabled: hook.get("enabled")?.as_bool()?,
        trust_status: hook.get("trustStatus")?.as_str()?.to_string(),
        source_path: source_path.to_string(),
        current_hash: hook
            .get("currentHash")
            .and_then(Value::as_str)
            .map(str::to_string),
        observed_at_ms: now_epoch_ms(),
    })
}

fn same_path(left: &Path, right: &Path) -> bool {
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => left == right,
    }
}

pub(super) fn hook_health(probe: Option<&CodexHookProbe>, active: bool) -> (HealthStatus, String) {
    match probe {
        Some(probe) if !probe.enabled => (
            HealthStatus::Unavailable,
            "permission hook installed but disabled".to_string(),
        ),
        Some(probe) if probe.trust_status != "trusted" => (
            HealthStatus::Degraded,
            format!("permission hook awaiting trust ({})", probe.trust_status),
        ),
        Some(_) if active => (
            HealthStatus::Connected,
            "permission hook trusted and exchange observed".to_string(),
        ),
        Some(_) => (
            HealthStatus::Degraded,
            "permission hook trusted; no exchange observed yet".to_string(),
        ),
        None => (
            HealthStatus::Unavailable,
            "permission hook not discovered".to_string(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_native_codex_hook_trust_state() {
        let hook = json!({
            "command": "\"/Users/me/.codex/cc-autobahn/cc-autobahn-codex-permission-hook\" permission-hook codex",
            "eventName": "permissionRequest",
            "handlerType": "command",
            "source": "user",
            "enabled": true,
            "trustStatus": "untrusted",
            "sourcePath": "/Users/me/.codex/hooks.json",
            "currentHash": "sha256:abc"
        });
        let probe =
            normalize_hook_probe(&hook, Some(Path::new("/Users/me/.codex/hooks.json"))).unwrap();
        assert!(probe.enabled);
        assert_eq!(probe.trust_status, "untrusted");
        assert_eq!(probe.current_hash.as_deref(), Some("sha256:abc"));
    }

    #[test]
    fn disabled_or_modified_hook_never_becomes_connected_from_old_activity() {
        let mut probe = CodexHookProbe {
            enabled: false,
            trust_status: "trusted".into(),
            source_path: "/tmp/hooks.json".into(),
            current_hash: Some("sha256:abc".into()),
            observed_at_ms: 1,
        };
        assert_eq!(hook_health(Some(&probe), true).0, HealthStatus::Unavailable);
        probe.enabled = true;
        probe.trust_status = "modified".into();
        assert_eq!(hook_health(Some(&probe), true).0, HealthStatus::Degraded);
    }

    #[test]
    fn normalizes_selected_codex_bucket_and_preserves_all_buckets() {
        let value = json!({
            "rateLimits": { "primary": { "usedPercent": 99 } },
            "rateLimitsByLimitId": {
                "other": { "limitId": "other", "primary": { "usedPercent": 10 } },
                "codex": {
                    "limitId": "codex",
                    "limitName": "Codex",
                    "planType": "plus",
                    "primary": { "usedPercent": 23, "windowDurationMins": 300, "resetsAt": 1800000000 },
                    "secondary": { "usedPercent": 41, "windowDurationMins": 10080 }
                }
            }
        });
        let snapshot = normalize_limits(&value, SourceQuality::Official).unwrap();
        assert_eq!(snapshot.primary.unwrap().used_percent, 23.0);
        assert_eq!(
            snapshot.secondary.unwrap().window_duration_minutes,
            Some(10080)
        );
        assert_eq!(snapshot.buckets.len(), 2);
        assert_eq!(snapshot.buckets[0].limit_id.as_deref(), Some("codex"));
    }

    #[test]
    fn sparse_update_keeps_old_values_and_ignores_nulls() {
        let mut current = Some(json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "usedPercent": 20, "windowDurationMins": 300, "resetsAt": 100 }
            },
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "primary": { "usedPercent": 20, "windowDurationMins": 300, "resetsAt": 100 }
                }
            }
        }));
        merge_rate_limit_update(
            &mut current,
            &json!({ "limitId": "codex", "primary": { "usedPercent": 35, "resetsAt": null } }),
        );
        let snapshot =
            normalize_limits(current.as_ref().unwrap(), SourceQuality::Official).unwrap();
        let primary = snapshot.primary.unwrap();
        assert_eq!(primary.used_percent, 35.0);
        assert_eq!(primary.window_duration_minutes, Some(300));
        assert_eq!(primary.resets_at_ms, Some(100_000));
    }

    #[test]
    fn sparse_update_builds_multi_bucket_map_after_null_snapshot_field() {
        let mut current = Some(json!({
            "rateLimits": {},
            "rateLimitsByLimitId": null
        }));
        merge_rate_limit_update(
            &mut current,
            &json!({
                "limitId": "codex",
                "primary": { "usedPercent": 12 }
            }),
        );
        let snapshot =
            normalize_limits(current.as_ref().unwrap(), SourceQuality::Official).unwrap();
        assert_eq!(snapshot.primary.unwrap().used_percent, 12.0);
        assert_eq!(snapshot.buckets.len(), 1);
    }

    #[test]
    fn update_for_another_bucket_preserves_the_legacy_codex_bucket() {
        let mut current = Some(json!({
            "rateLimits": {
                "limitId": "codex",
                "primary": { "usedPercent": 20 }
            },
            "rateLimitsByLimitId": null
        }));
        merge_rate_limit_update(
            &mut current,
            &json!({
                "limitId": "other",
                "primary": { "usedPercent": 80 }
            }),
        );
        let snapshot =
            normalize_limits(current.as_ref().unwrap(), SourceQuality::Official).unwrap();
        assert_eq!(snapshot.primary.unwrap().used_percent, 20.0);
        assert_eq!(snapshot.buckets.len(), 2);
        assert_eq!(snapshot.buckets[0].limit_id.as_deref(), Some("codex"));
        assert_eq!(snapshot.buckets[1].limit_id.as_deref(), Some("other"));
    }

    #[test]
    fn sparse_update_without_id_merges_into_unambiguous_codex_bucket() {
        let mut current = Some(json!({
            "rateLimits": { "primary": { "usedPercent": 10 } },
            "rateLimitsByLimitId": {
                "codex": {
                    "limitId": "codex",
                    "primary": { "usedPercent": 10, "windowDurationMins": 300 }
                }
            }
        }));
        merge_rate_limit_update(&mut current, &json!({ "primary": { "usedPercent": 45 } }));
        let snapshot =
            normalize_limits(current.as_ref().unwrap(), SourceQuality::Official).unwrap();
        assert_eq!(snapshot.primary.unwrap().used_percent, 45.0);
        assert_eq!(
            snapshot.buckets[0].primary.as_ref().unwrap().used_percent,
            45.0
        );
    }

    #[test]
    fn account_usage_is_official_but_contains_no_billing_claim() {
        let value = json!({
            "summary": { "lifetimeTokens": 1234, "peakDailyTokens": 500 },
            "dailyUsageBuckets": [{ "startDate": "2026-07-19", "tokens": 42 }]
        });
        let snapshot = normalize_usage(&value, SourceQuality::Official).unwrap();
        assert_eq!(snapshot.lifetime_tokens, Some(1234));
        assert_eq!(snapshot.daily_usage[0].tokens, 42);
    }

    #[test]
    fn incompatible_success_shapes_are_not_treated_as_capability_success() {
        assert!(normalize_limits(&json!({}), SourceQuality::Official).is_none());
        assert!(normalize_usage(&json!({}), SourceQuality::Official).is_none());
    }
}
