//! statusline mode — CLI: stdin → (previous chain to stdout) + sensor file.
//! Entry point for `cc-autobahn statusline`, invoked by Claude Code itself.

use std::fs;
use std::io::{Read, Write};
use std::process::{Command, Stdio};

use super::{prev_statusline_file, status_file, write_private, StatusInput};

/// Entry point of `statusline` mode (`argv[1] == "statusline"`). Reads the
/// session JSON from stdin, re-emits the user's previous statusLine (chain,
/// D12/D-new-3) or a default line, and dumps the JSON to the sensor file.
/// Always exits successfully (a failing statusline messes up the terminal).
pub fn run_statusline() {
    let mut buf = Vec::new();
    let _ = std::io::stdin().read_to_end(&mut buf);

    if !chain_prev_statusline(&buf) {
        print_default_line(&buf);
    }
    write_status_file(&buf);
    let _ = std::io::stdout().flush();
}

/// Re-runs the previous statusLine (saved in `cc-autobahn/prev-statusline`) with
/// `buf` as stdin and re-emits its stdout. `true` if it emitted something. macOS-first: uses
/// `/bin/sh`; on Windows the spawn fails and it falls back to the default line.
fn chain_prev_statusline(buf: &[u8]) -> bool {
    let Some(cmd_path) = prev_statusline_file() else {
        return false;
    };
    let Ok(cmd) = fs::read_to_string(&cmd_path) else {
        return false;
    };
    let cmd = cmd.trim();
    if cmd.is_empty() {
        return false;
    }
    let Ok(mut child) = Command::new("/bin/sh")
        .arg("-c")
        .arg(cmd)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    // The session JSON comfortably fits the kernel pipe; the previous statusLine
    // either reads it or ignores it. If it ignores it, write_all still finishes.
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(buf);
    }
    let Ok(output) = child.wait_with_output() else {
        return false;
    };
    if output.stdout.is_empty() {
        return false;
    }
    let _ = std::io::stdout().write_all(&output.stdout);
    true
}

/// Default line when there's no previous statusLine or the chain failed.
fn print_default_line(buf: &[u8]) {
    let parsed: StatusInput = serde_json::from_slice(buf).unwrap_or_default();
    let model = parsed
        .model
        .as_ref()
        .and_then(|m| m.display_name.clone().or_else(|| m.id.clone()))
        .unwrap_or_else(|| "claude".to_string());
    let cost = parsed
        .cost
        .as_ref()
        .and_then(|c| c.total_cost_usd)
        .map(|v| format!(" · ${v:.2}"))
        .unwrap_or_default();
    println!("cc-autobahn · {model}{cost}");
}

/// Writes `buf` to the sensor file via tmp write + atomic rename (mode 0600).
/// Discards entries that aren't valid JSON (avoid corrupting the tail).
fn write_status_file(buf: &[u8]) {
    let Some(path) = status_file() else {
        return;
    };
    let Some(dir) = path.parent() else {
        return;
    };
    if serde_json::from_slice::<serde_json::Value>(buf).is_err() {
        return;
    }
    let _ = fs::create_dir_all(dir);
    let tmp = path.with_extension("json.tmp");
    if write_private(&tmp, buf) {
        let _ = fs::rename(&tmp, &path);
    }
}
