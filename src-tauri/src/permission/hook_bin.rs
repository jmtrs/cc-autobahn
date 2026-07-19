//! `cc-autobahn permission-hook` — CLI entrypoint for Claude Code's
//! `PermissionRequest` hook (D42). Reads the request from stdin, blocks on
//! the GUI's socket for a human decision, and prints the `hookSpecificOutput`
//! JSON Claude Code expects.
//!
//! On ANY failure (no socket, malformed input, nobody answered in time)
//! prints NOTHING and exits 0 — that silence is itself the fail-open signal
//! Claude Code's own hook contract already defines: no valid JSON on stdout
//! means it falls back to its normal terminal permission prompt. This never
//! invents a decision on its own initiative; a dead cc-autobahn must never
//! hang or silently gate a real coding session.

use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

use serde::Deserialize;

use super::{socket_path, Decision, DecisionMsg, HookRequest};

/// Just under Claude Code's own 600s hook timeout, so this always loses the
/// race and prints nothing rather than being killed mid-write.
const HOOK_READ_TIMEOUT_SECS: u64 = 580;

/// Native permission suggestions can be almost as large as the incoming hook
/// payload because the GUI echoes one unchanged as `updatedPermissions`.
/// Preserve the same hard cap plus small JSON-envelope headroom.
const MAX_RESPONSE_BYTES: u64 = super::MAX_REQUEST_BYTES + 4 * 1024;

/// Claude Code's own stdin contract for tool-event hooks (snake_case) —
/// distinct from [`HookRequest`], which is this app's own (camelCase) wire
/// format for the socket leg to the GUI.
#[derive(Debug, Deserialize)]
struct CcHookInput {
    session_id: String,
    #[serde(default)]
    prompt_id: Option<String>,
    tool_name: String,
    #[serde(default)]
    tool_input: serde_json::Value,
    #[serde(default)]
    cwd: String,
    #[serde(default)]
    permission_suggestions: Vec<serde_json::Value>,
}

/// Generates an invocation identity independent of Claude's prompt
/// correlation. Every hook invocation is a separate process, so process-local
/// counters and PIDs cannot provide uniqueness after PID reuse.
fn generate_request_id() -> String {
    format!("claude-{}", uuid::Uuid::new_v4())
}

/// Entry point of `permission-hook` mode (`argv[1] == "permission-hook"`).
/// Never panics on malformed stdin (mirrors `run_statusline`'s "always exits
/// successfully" — a hook that crashes or hangs is worse than one that
/// silently no-ops).
pub fn run_permission_hook() {
    let mut buf = Vec::new();
    if std::io::stdin().read_to_end(&mut buf).is_err() {
        return;
    }
    let Ok(input) = serde_json::from_slice::<CcHookInput>(&buf) else {
        return;
    };
    let request = HookRequest {
        request_id: generate_request_id(),
        prompt_id: input.prompt_id,
        session_id: input.session_id,
        tool_name: input.tool_name,
        tool_input: input.tool_input,
        cwd: input.cwd,
        permission_suggestions: input.permission_suggestions,
    };

    let Some(decision) = ask_gui(&request) else {
        return; // no GUI running, or nobody answered in time — fail open, print nothing
    };
    print_decision(decision);
}

/// Connects to the GUI's socket, sends the request, and blocks for a
/// decision. `None` on any failure (including timeout) — the caller's
/// silence is the fail-open behavior.
#[cfg(unix)]
fn ask_gui(request: &HookRequest) -> Option<DecisionMsg> {
    use std::os::unix::net::UnixStream;

    let path = socket_path()?;
    // AF_UNIX `connect()` against a missing path, or a stale path with no
    // listener, fails near-instantly (ENOENT/ECONNREFUSED) — there's no
    // slow-handshake failure mode to guard against here, unlike a network
    // socket, so no manual connect-timeout wrapper is needed.
    let mut stream = UnixStream::connect(path).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(HOOK_READ_TIMEOUT_SECS)))
        .ok()?;
    // Bounds the request write the same way the read is bounded — without
    // this a stalled GUI-side reader (frozen, not crashed) could block
    // write_all past the documented fail-open budget instead of erroring out.
    stream
        .set_write_timeout(Some(Duration::from_secs(HOOK_READ_TIMEOUT_SECS)))
        .ok()?;

    let mut line = serde_json::to_string(request).ok()?;
    line.push('\n');
    stream.write_all(line.as_bytes()).ok()?;

    let mut reader = BufReader::new(stream).take(MAX_RESPONSE_BYTES);
    let mut response = String::new();
    if reader.read_line(&mut response).ok()? == 0 {
        return None;
    }
    let msg: super::DecisionMsg = serde_json::from_str(&response).ok()?;
    Some(msg)
}

#[cfg(not(unix))]
fn ask_gui(_request: &HookRequest) -> Option<DecisionMsg> {
    None // the permission hook isn't wired on non-unix targets — fail open.
}

/// Builds the `hookSpecificOutput` JSON for `PermissionRequest`. The
/// decision is a NESTED `decision.behavior` field, not the flat
/// `permissionDecision` string that `PreToolUse` hooks use — confirmed
/// against the official schema (code.claude.com/docs/en/hooks.md). Getting
/// this wrong is silent: Claude Code just doesn't recognize the output and
/// falls back to its own terminal prompt, which is exactly what happened
/// before this was fixed (D42 follow-up bug). PURE → testable without stdout.
fn decision_output(response: DecisionMsg) -> serde_json::Value {
    let behavior = match response.decision {
        Decision::Allow => "allow",
        Decision::Deny => "deny",
    };
    let mut decision = serde_json::json!({ "behavior": behavior });
    if !response.updated_permissions.is_empty() {
        decision["updatedPermissions"] = serde_json::Value::Array(response.updated_permissions);
    }
    serde_json::json!({
        "hookSpecificOutput": {
            "hookEventName": "PermissionRequest",
            "decision": decision,
        }
    })
}

/// Writes the decision JSON directly instead of `println!`/`print!` — those
/// macros `panic!` on a stdout write failure (e.g. Claude Code already gave
/// up reading and closed the pipe, EPIPE), which would violate this file's
/// own "never panics, worst case is silence" contract right at the one
/// moment a real decision was about to be delivered. `writeln!` on a
/// `Write` value returns a `Result` instead, so a failed write is just
/// ignored like every other fallible step in this file.
fn print_decision(response: DecisionMsg) {
    let out = decision_output(response);
    let mut stdout = std::io::stdout();
    let _ = writeln!(stdout, "{out}");
    let _ = stdout.flush();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allow_uses_nested_behavior_shape() {
        let v = decision_output(DecisionMsg::plain(Decision::Allow));
        assert_eq!(
            v["hookSpecificOutput"]["hookEventName"],
            "PermissionRequest"
        );
        assert_eq!(v["hookSpecificOutput"]["decision"]["behavior"], "allow");
        assert!(
            v["hookSpecificOutput"].get("permissionDecision").is_none(),
            "must not regress to the flat PreToolUse-shaped field"
        );
    }

    #[test]
    fn deny_uses_nested_behavior_shape() {
        let v = decision_output(DecisionMsg::plain(Decision::Deny));
        assert_eq!(v["hookSpecificOutput"]["decision"]["behavior"], "deny");
    }

    #[test]
    fn native_permission_suggestion_round_trips_unchanged() {
        let suggestion = serde_json::json!({
            "type": "addRules",
            "rules": [{ "toolName": "Bash", "ruleContent": "npm test" }],
            "behavior": "allow",
            "destination": "localSettings"
        });
        let v = decision_output(DecisionMsg::allow_with_permissions(
            vec![suggestion.clone()],
        ));
        assert_eq!(
            v["hookSpecificOutput"]["decision"]["updatedPermissions"][0],
            suggestion
        );
    }

    #[test]
    fn prompt_id_is_optional_and_request_ids_are_distinct() {
        let input: CcHookInput = serde_json::from_value(serde_json::json!({
            "session_id": "session-1",
            "tool_name": "Bash",
            "tool_input": { "command": "npm test" }
        }))
        .unwrap();
        assert!(input.prompt_id.is_none());
        assert_ne!(generate_request_id(), generate_request_id());
    }

    #[test]
    fn response_limit_preserves_native_suggestion_larger_than_4kib() {
        let suggestion = serde_json::json!({
            "type": "addRules",
            "rules": [{ "toolName": "Bash", "ruleContent": "x".repeat(8 * 1024) }],
            "behavior": "allow",
            "destination": "localSettings"
        });
        let response = DecisionMsg::allow_with_permissions(vec![suggestion.clone()]);
        let mut wire = serde_json::to_string(&response).unwrap();
        wire.push('\n');
        assert!(wire.len() > 4 * 1024);
        assert!((wire.len() as u64) < MAX_RESPONSE_BYTES);

        let mut reader = BufReader::new(std::io::Cursor::new(wire)).take(MAX_RESPONSE_BYTES);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        let decoded: DecisionMsg = serde_json::from_str(&line).unwrap();
        assert_eq!(decoded.updated_permissions, vec![suggestion]);
    }
}
