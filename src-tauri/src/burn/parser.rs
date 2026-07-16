//! Turn calculation — PURE LOGIC (no Tauri) → testable.
//! Parses one JSONL line at a time and, when a turn closes (or an
//! intermediate tool-use message arrives, D27), computes `tok/s`.

use std::collections::HashSet;

use serde::Deserialize;

use super::zulu::parse_zulu_millis;

// ─────────────────────────────────────────────────────────────────────────────
// Serde model of a JSONL line (only what we use)
// ─────────────────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct JsonlLine {
    #[serde(default, rename = "type")]
    kind: String,
    #[serde(default)]
    timestamp: String,
    #[serde(default)]
    message: Option<AssistantMsg>,
}

#[derive(Deserialize)]
struct AssistantMsg {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    stop_reason: Option<String>,
    #[serde(default)]
    usage: Option<Usage>,
}

#[derive(Deserialize, Default)]
struct Usage {
    #[serde(default)]
    output_tokens: u64,
}

/// Closure of a turn: ready to emit as `burn-tick`.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct BurnCalc {
    pub(crate) tok_per_s: f64,
    pub(crate) turn_output_tokens: u64,
    pub(crate) turn_duration_ms: i64,
    pub(crate) message_id: String,
    pub(crate) timestamp: String,
}

/// State of the current turn. Accumulates `output_tokens` (deduped by `message.id`)
/// from the previous closure until the next `end_turn`/`stop_sequence`.
#[derive(Default)]
pub(crate) struct TurnState {
    /// `ts` of the previous turn's closure (None at startup).
    last_end_ms: Option<i64>,
    /// Σ deduplicated `output_tokens` of the current turn.
    turn_output: u64,
    /// `ts` of the first accumulated message of the turn (Δt fallback if there's no
    /// previous closure — e.g. when hooking into a file mid-session).
    turn_start_ms: Option<i64>,
    /// `ts` of the last message seen (any, not just closures) — the base for the
    /// Δt of intermediate partial ticks. `None` when each turn starts.
    last_msg_ms: Option<i64>,
    /// `message.id`s already counted (discards rewrites, which carry the same value).
    seen: HashSet<String>,
}

impl TurnState {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Ingests an assistant message. Returns `Some(BurnCalc)` in two cases:
    /// (a) this message closes the turn (`end_turn`/`stop_sequence`, aggregate
    /// of the WHOLE turn) or (b) it's an intermediate message (`tool_use`, etc.) that
    /// isn't the first of the turn — PARTIAL tick with only its own
    /// tokens/Δt, so as not to wait for closure in long turns with tool use.
    fn ingest(
        &mut self,
        msg_id: &str,
        out_tok: u64,
        ts_ms: i64,
        stop_reason: Option<&str>,
        timestamp: &str,
    ) -> Option<BurnCalc> {
        // Tokens counted ONLY the first time we see the id (rewrites
        // carry the same value, D8). Closure is always processed: if an id seen
        // as `tool_use` reappears as `end_turn`, the turn must close regardless.
        let first_time = self.seen.insert(msg_id.to_string());
        if first_time {
            if self.turn_start_ms.is_none() {
                self.turn_start_ms = Some(ts_ms);
            }
            self.turn_output += out_tok;
        }

        let closes = matches!(stop_reason, Some("end_turn") | Some("stop_sequence"));

        if !closes {
            // Intermediate message: partial tick only if it's the first time we
            // see it (a rewrite isn't new work) and there's real Δt — the
            // turn's first message always has Δt=0 against itself, so
            // it doesn't emit (correct: there's nothing to measure yet).
            if !first_time || out_tok == 0 {
                return None;
            }
            let start_ms = self.last_msg_ms.or(self.turn_start_ms).unwrap_or(ts_ms);
            let dt_ms = ts_ms - start_ms;
            if dt_ms <= 0 {
                // ts not monotonic: do NOT seal last_msg_ms (same as last_end_ms on
                // closure) so as not to lose the reference for the next Δt.
                return None;
            }
            self.last_msg_ms = Some(ts_ms);
            return Some(BurnCalc {
                tok_per_s: out_tok as f64 * 1000.0 / dt_ms as f64,
                turn_output_tokens: out_tok,
                turn_duration_ms: dt_ms,
                message_id: msg_id.to_string(),
                timestamp: timestamp.to_string(),
            });
        }

        // `turn_output == 0` discards: empty turns (0 tokens) and rewrites of
        // an already-emitted closure (which would leave the accumulator at 0 after reset).
        if self.turn_output == 0 {
            return None;
        }

        // Δt = current closure − previous closure (or, failing that, turn start).
        let start_ms = self.last_end_ms.or(self.turn_start_ms).unwrap_or(ts_ms);
        let dt_ms = ts_ms - start_ms;
        if dt_ms <= 0 {
            // ts not monotonic (rewrite/clock glitch): don't emit OR seal the closure,
            // so as not to lose accumulated tokens or skew the next Δt.
            return None;
        }

        let calc = BurnCalc {
            tok_per_s: self.turn_output as f64 * 1000.0 / dt_ms as f64,
            turn_output_tokens: self.turn_output,
            turn_duration_ms: dt_ms,
            message_id: msg_id.to_string(),
            timestamp: timestamp.to_string(),
        };
        // Seal the closure ONLY when emitting: the next turn starts from here.
        self.last_end_ms = Some(ts_ms);
        self.turn_output = 0;
        self.turn_start_ms = None;
        self.last_msg_ms = None;
        Some(calc)
    }
}

/// Processes a raw JSONL line. `Some(BurnCalc)` if it closes a turn.
pub(crate) fn process_line(state: &mut TurnState, line: &[u8]) -> Option<BurnCalc> {
    let parsed: JsonlLine = serde_json::from_slice(line).ok()?;
    if parsed.kind != "assistant" {
        return None;
    }
    let msg = parsed.message?;
    let usage = msg.usage?;
    let msg_id = msg.id?;
    let ts_ms = parse_zulu_millis(&parsed.timestamp)?;
    state.ingest(
        &msg_id,
        usage.output_tokens,
        ts_ms,
        msg.stop_reason.as_deref(),
        &parsed.timestamp,
    )
}

/// Accumulates `chunk` into `leftover` and returns the complete lines (without `\n`),
/// leaving the remainder without `\n` in `leftover` for the next cycle. This way a chunk
/// that cuts a line halfway is reassembled when the next batch arrives.
pub(crate) fn split_lines(leftover: &mut Vec<u8>, chunk: &[u8]) -> Vec<Vec<u8>> {
    leftover.extend_from_slice(chunk);
    let bytes = std::mem::take(leftover);
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &byte) in bytes.iter().enumerate() {
        if byte == b'\n' {
            out.push(bytes[start..i].to_vec());
            start = i + 1;
        }
    }
    *leftover = bytes[start..].to_vec();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal valid assistant line for the parser.
    fn assistant(id: &str, ts: &str, out: u64, stop: &str) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","stop_reason":"{stop}","usage":{{"output_tokens":{out}}}}}}}"#
        )
    }

    #[test]
    fn turn_calc_first_turn_uses_turn_start() {
        // First turn: no previous closure, Δt = from the first message.
        let mut s = TurnState::new();
        let a = assistant("m1", "2026-07-16T08:00:00.000Z", 200, "tool_use");
        let b = assistant("m2", "2026-07-16T08:00:10.000Z", 300, "end_turn");
        assert!(process_line(&mut s, a.as_bytes()).is_none()); // tool_use doesn't close
        let calc = process_line(&mut s, b.as_bytes()).expect("closes turn");
        // Δoutput=500, Δt=10 s → 50 tok/s
        assert_eq!(calc.turn_output_tokens, 500);
        assert_eq!(calc.turn_duration_ms, 10_000);
        assert!((calc.tok_per_s - 50.0).abs() < 1e-6, "tok/s = {}", calc.tok_per_s);
    }

    #[test]
    fn turn_calc_second_turn_uses_last_end() {
        let mut s = TurnState::new();
        process_line(&mut s, assistant("m1", "2026-07-16T08:00:00.000Z", 200, "tool_use").as_bytes());
        process_line(&mut s, assistant("m2", "2026-07-16T08:00:10.000Z", 300, "end_turn").as_bytes());
        // Second turn: previous closure = 08:00:10.
        process_line(&mut s, assistant("m3", "2026-07-16T08:00:15.000Z", 100, "tool_use").as_bytes());
        let calc = process_line(&mut s, assistant("m4", "2026-07-16T08:00:35.000Z", 400, "end_turn").as_bytes())
            .expect("closes second turn");
        // Δoutput=500, Δt=08:00:35 − 08:00:10 = 25 s → 20 tok/s
        assert_eq!(calc.turn_output_tokens, 500);
        assert_eq!(calc.turn_duration_ms, 25_000);
        assert!((calc.tok_per_s - 20.0).abs() < 1e-6);
    }

    #[test]
    fn intermediate_tool_use_emits_partial_tick() {
        // Turn with 2 tool_use before closure: the FIRST has Δt=0 (turn
        // start, nothing to measure yet) but the SECOND does emit a
        // partial tick with ONLY its own tokens/Δt (not accumulated), without waiting
        // for final closure — earlier feedback in long turns with
        // tool use (D27).
        let mut s = TurnState::new();
        let a1 = assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use");
        let a2 = assistant("a2", "2026-07-16T08:00:05.000Z", 150, "tool_use");
        assert!(
            process_line(&mut s, a1.as_bytes()).is_none(),
            "first message of the turn, Δt=0 against itself"
        );
        let partial = process_line(&mut s, a2.as_bytes())
            .expect("second intermediate message emits partial tick");
        // Δoutput = 150 (ONLY a2, not accumulated with a1), Δt = 5 s → 30 tok/s
        assert_eq!(partial.turn_output_tokens, 150);
        assert_eq!(partial.turn_duration_ms, 5_000);
        assert!((partial.tok_per_s - 30.0).abs() < 1e-6);

        // The final closure DOES aggregate the WHOLE turn (100+150+200=450), not just
        // what remained since the last partial tick.
        let close = process_line(
            &mut s,
            assistant("a3", "2026-07-16T08:00:15.000Z", 200, "end_turn").as_bytes(),
        )
        .expect("closes turn");
        assert_eq!(close.turn_output_tokens, 450);
        assert_eq!(close.turn_duration_ms, 15_000); // from the start of the turn
        assert!((close.tok_per_s - 30.0).abs() < 1e-6); // 450 / 15
    }

    #[test]
    fn intermediate_dt_non_monotonic_does_not_seal_last_msg() {
        // Mirror of `dt_non_monotonic_does_not_reset` but for the
        // PARTIAL tick: an intermediate message with non-monotonic ts must not seal
        // `last_msg_ms`, or the next partial tick would calculate its Δt against
        // an incorrect reference (bug found in code review, D27).
        let mut s = TurnState::new();
        let a1 = assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use");
        assert!(process_line(&mut s, a1.as_bytes()).is_none(), "Δt=0, start of the turn");

        // a2 arrives with ts EARLIER than a1 (rewrite/clock glitch) → Δt<0 → None,
        // and must NOT seal last_msg_ms with this erroneous ts.
        let bad = assistant("a2", "2026-07-16T07:59:00.000Z", 50, "tool_use");
        assert!(process_line(&mut s, bad.as_bytes()).is_none(), "non-monotonic ts doesn't emit");

        // a3 arrives correctly 5s after a1 (NOT after a2): if last_msg_ms had
        // been sealed with a2's ts, Δt would be absurdly large.
        let a3 = assistant("a3", "2026-07-16T08:00:05.000Z", 150, "tool_use");
        let partial = process_line(&mut s, a3.as_bytes())
            .expect("partial tick uses the correct reference (a1, not a2)");
        assert_eq!(partial.turn_duration_ms, 5_000); // 08:00:05 − 08:00:00, not vs 07:59:00
        assert!((partial.tok_per_s - 30.0).abs() < 1e-6); // 150 tok / 5 s
    }

    #[test]
    fn dedup_by_message_id() {
        // Rewrite of the same message.id → not counted twice.
        let mut s = TurnState::new();
        let first = assistant("m1", "2026-07-16T08:00:00.000Z", 200, "tool_use");
        let rewrite = assistant("m1", "2026-07-16T08:00:01.000Z", 200, "tool_use");
        assert!(process_line(&mut s, first.as_bytes()).is_none());
        assert!(process_line(&mut s, rewrite.as_bytes()).is_none()); // ignored
        let calc = process_line(&mut s, assistant("m2", "2026-07-16T08:00:10.000Z", 300, "end_turn").as_bytes())
            .unwrap();
        // 200 (once) + 300 = 500, not 700.
        assert_eq!(calc.turn_output_tokens, 500);
    }

    #[test]
    fn ignores_non_assistant_and_partial() {
        let mut s = TurnState::new();
        // user / system / garbage → none close.
        assert!(process_line(&mut s, br#"{"type":"user","timestamp":"2026-07-16T08:00:00.000Z"}"#).is_none());
        assert!(process_line(&mut s, b"this is not json").is_none());
        assert!(process_line(&mut s, b"").is_none());
        // assistant without usage → ignored.
        assert!(process_line(&mut s, br#"{"type":"assistant","timestamp":"2026-07-16T08:00:00.000Z","message":{"id":"x"}}"#).is_none());
    }

    #[test]
    fn dedup_does_not_swallow_closure() {
        // BUG 2: an id seen as tool_use and rewritten as end_turn must NOT
        // ignore the closure. Tokens are counted the first time (100).
        let mut s = TurnState::new();
        process_line(&mut s, assistant("mx", "2026-07-16T08:00:00.000Z", 100, "tool_use").as_bytes());
        let calc = process_line(
            &mut s,
            assistant("mx", "2026-07-16T08:00:10.000Z", 200, "end_turn").as_bytes(),
        )
        .expect("the closure must not be swallowed");
        assert_eq!(calc.turn_output_tokens, 100); // counted only once
    }

    #[test]
    fn dt_non_monotonic_does_not_reset() {
        // RISK 6: a closure with non-monotonic ts doesn't emit, does NOT update
        // last_end_ms (doesn't go backwards), and does NOT lose accumulated tokens.
        let mut s = TurnState::new();
        // Initial VALID turn (with prior tool_use → dt>0) to fix last_end_ms.
        process_line(&mut s, assistant("a1", "2026-07-16T08:00:00.000Z", 100, "tool_use").as_bytes());
        process_line(&mut s, assistant("a2", "2026-07-16T08:00:10.000Z", 50, "end_turn").as_bytes());
        // Turn in progress: tool_use accumulates 300.
        process_line(&mut s, assistant("t1", "2026-07-16T08:00:15.000Z", 300, "tool_use").as_bytes());
        // closure with ts EARLIER than the last valid closure (08:00:10) → dt < 0 → None.
        let bad = process_line(
            &mut s,
            assistant("c2", "2026-07-16T07:59:00.000Z", 0, "stop_sequence").as_bytes(),
        );
        assert!(bad.is_none(), "non-monotonic ts doesn't emit");
        // correct later closure: Δt uses the original last_end_ms (08:00:10), not 07:59:00.
        let good = process_line(
            &mut s,
            assistant("c3", "2026-07-16T08:00:30.000Z", 10, "end_turn").as_bytes(),
        )
        .expect("closes with tokens preserved");
        assert_eq!(good.turn_duration_ms, 20_000); // 08:00:30 − 08:00:10, not 90 s
        assert_eq!(good.turn_output_tokens, 310); // 300 (t1) + 10 (c3); c2 contributed 0
    }

    #[test]
    fn split_lines_handles_partial_writes() {
        let mut leftover = Vec::new();
        // chunk 1: "abc\ndef" → "def" stays as leftover (without \n).
        let l1 = split_lines(&mut leftover, b"abc\ndef");
        assert_eq!(l1, vec![b"abc".to_vec()]);
        assert_eq!(leftover, b"def");
        // chunk 2: "\nghi" → completes "def" WITHOUT duplicating it.
        let l2 = split_lines(&mut leftover, b"\nghi");
        assert_eq!(l2, vec![b"def".to_vec()]);
        assert_eq!(leftover, b"ghi");
    }
}
