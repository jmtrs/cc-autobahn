//! JSON-RPC framing for the Codex App Server stdio child.
//!
//! Reads newline-delimited JSON messages off the child's stdout (bounded so a
//! runaway line can't exhaust memory) and writes requests/notifications to its
//! stdin. Correlation and dispatch stay in the parent module; this layer only
//! owns the transport.

use std::io::{self, BufRead, Write};
use std::process::ChildStdin;
use std::sync::mpsc;

use serde_json::{json, Value};

const MAX_LINE_BYTES: usize = 1024 * 1024;

pub(super) enum ReaderEvent {
    Message(Value),
    Closed(String),
}

pub(super) fn send_request(
    stdin: &mut ChildStdin,
    id: u64,
    method: &str,
    params: Value,
) -> Result<(), String> {
    write_message(
        stdin,
        &json!({ "id": id, "method": method, "params": params }),
    )
}

pub(super) fn send_notification(stdin: &mut ChildStdin, method: &str) -> Result<(), String> {
    write_message(stdin, &json!({ "method": method }))
}

fn write_message(stdin: &mut ChildStdin, value: &Value) -> Result<(), String> {
    serde_json::to_writer(&mut *stdin, value).map_err(|error| error.to_string())?;
    stdin.write_all(b"\n").map_err(|error| error.to_string())?;
    stdin.flush().map_err(|error| error.to_string())
}

pub(super) fn read_messages<R: BufRead>(mut reader: R, sender: mpsc::Sender<ReaderEvent>) {
    loop {
        match read_bounded_line(&mut reader, MAX_LINE_BYTES) {
            Ok(Some(line)) => match serde_json::from_slice(&line) {
                Ok(message) => {
                    if sender.send(ReaderEvent::Message(message)).is_err() {
                        return;
                    }
                }
                Err(error) => {
                    if sender
                        .send(ReaderEvent::Closed(format!("invalid JSON-RPC: {error}")))
                        .is_err()
                    {
                        return;
                    }
                    return;
                }
            },
            Ok(None) => {
                let _ = sender.send(ReaderEvent::Closed("App Server closed stdout".into()));
                return;
            }
            Err(error) => {
                let _ = sender.send(ReaderEvent::Closed(error.to_string()));
                return;
            }
        }
    }
}

fn read_bounded_line<R: BufRead>(reader: &mut R, limit: usize) -> io::Result<Option<Vec<u8>>> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(None)
            } else {
                Ok(Some(line))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let take = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(take) > limit {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "App Server message exceeds size limit",
            ));
        }
        line.extend_from_slice(&available[..take]);
        reader.consume(take);
        if newline.is_some() {
            while matches!(line.last(), Some(b'\n' | b'\r')) {
                line.pop();
            }
            return Ok(Some(line));
        }
    }
}

pub(super) fn response_id(message: &Value) -> Option<u64> {
    if message.get("result").is_none() && message.get("error").is_none() {
        return None;
    }
    message.get("id")?.as_u64()
}

pub(super) fn compact_error(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("request rejected")
        .chars()
        .take(180)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::BufReader;

    #[test]
    fn server_request_cannot_spoof_a_correlated_response() {
        assert_eq!(
            response_id(&json!({ "id": 1, "method": "item/commandExecution/requestApproval" })),
            None
        );
        assert_eq!(response_id(&json!({ "id": 1, "result": {} })), Some(1));
    }

    #[test]
    fn bounded_reader_rejects_oversized_message() {
        let input = vec![b'x'; 9];
        let error = read_bounded_line(&mut BufReader::new(input.as_slice()), 8).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}
