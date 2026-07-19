//! Defensive discovery and tailing for Codex rollout JSONL.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tauri::{AppHandle, Emitter};

use crate::burn::zulu::parse_zulu_millis;
use crate::providers::{emit_model_activity, ModelActivity, ProviderId, TurnRate};

const ACTIVE_WINDOW: Duration = Duration::from_secs(60 * 60);
const MAX_DEPTH: usize = 8;
const MAX_ACTIVE_FILES: usize = 512;
const MAX_READ_BYTES: u64 = 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;
const BOOTSTRAP_TAIL_BYTES: u64 = 1024 * 1024;
const BOOTSTRAP_HEAD_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq)]
enum DecodedEvent {
    Rate(TurnRate),
    Model(ModelActivity),
}

#[derive(Default)]
struct Decoder {
    thread_id: Option<String>,
    turn_id: Option<String>,
    current_model: Option<String>,
    model_observed_at_ms: Option<i64>,
    turn_started_at_ms: Option<i64>,
    last_response_at_ms: Option<i64>,
    last_total_output: Option<u64>,
    sequence: u64,
}

impl Decoder {
    fn process_line(&mut self, line: &[u8]) -> Option<DecodedEvent> {
        if line.is_empty() || line.len() > MAX_LINE_BYTES {
            return None;
        }
        let record: Value = serde_json::from_slice(line).ok()?;
        let kind = record.get("type")?.as_str()?;
        let payload = record.get("payload")?;
        let observed_at_ms = record
            .get("timestamp")
            .and_then(Value::as_str)
            .and_then(parse_zulu_millis);

        match kind {
            "session_meta" => {
                self.thread_id = nonempty_string(payload.get("id"));
                None
            }
            "turn_context" => {
                let model_id = nonempty_string(payload.get("model"))?;
                if let Some(turn_id) = nonempty_string(payload.get("turn_id")) {
                    self.turn_id = Some(turn_id);
                }
                let thread_id = self.thread_id.clone()?;
                let observed_at_ms = observed_at_ms?;
                self.current_model = Some(model_id.clone());
                self.model_observed_at_ms = Some(observed_at_ms);
                self.sequence = self.sequence.wrapping_add(1);
                Some(DecodedEvent::Model(ModelActivity {
                    provider: ProviderId::Codex,
                    model_id,
                    session_or_thread_id: thread_id,
                    observed_at_ms,
                    sequence: self.sequence,
                }))
            }
            "event_msg" => match payload.get("type").and_then(Value::as_str) {
                Some("task_started") => {
                    self.turn_id = nonempty_string(payload.get("turn_id"));
                    self.turn_started_at_ms =
                        epoch_seconds_ms(payload.get("started_at")).or(observed_at_ms);
                    self.last_response_at_ms = self.turn_started_at_ms;
                    None
                }
                Some("task_complete") => {
                    self.turn_id = None;
                    self.turn_started_at_ms = None;
                    self.last_response_at_ms = None;
                    None
                }
                Some("token_count") => self.decode_token_count(payload, observed_at_ms?),
                _ => None,
            },
            _ => None,
        }
    }

    fn decode_token_count(&mut self, payload: &Value, observed_at_ms: i64) -> Option<DecodedEvent> {
        let info = payload.get("info")?.as_object()?;
        let output_tokens = info
            .get("last_token_usage")?
            .get("output_tokens")?
            .as_u64()?;
        let total_output = info
            .get("total_token_usage")
            .and_then(|usage| usage.get("output_tokens"))
            .and_then(Value::as_u64);

        if total_output.is_some() && total_output == self.last_total_output {
            return None;
        }
        self.last_total_output = total_output.or(self.last_total_output);

        let started_at_ms = self.last_response_at_ms.or(self.turn_started_at_ms)?;
        let elapsed_ms = observed_at_ms - started_at_ms;
        if output_tokens == 0 || elapsed_ms <= 0 {
            return None;
        }
        self.last_response_at_ms = Some(observed_at_ms);
        let tokens_per_second = output_tokens as f64 * 1000.0 / elapsed_ms as f64;
        if !tokens_per_second.is_finite() || tokens_per_second <= 0.0 {
            return None;
        }

        Some(DecodedEvent::Rate(TurnRate {
            provider: ProviderId::Codex,
            session_or_thread_id: self.thread_id.clone()?,
            observed_at_ms,
            output_tokens,
            elapsed_ms,
            tokens_per_second,
            partial: false,
        }))
    }
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn epoch_seconds_ms(value: Option<&Value>) -> Option<i64> {
    let seconds = value?.as_i64()?;
    seconds.checked_mul(1000)
}

pub(super) fn discover_rollout_roots() -> Vec<PathBuf> {
    let configured = crate::env_lock::var_os("CODEX_HOME")
        .map(|value| {
            value
                .to_string_lossy()
                .split(',')
                .map(str::trim)
                .filter(|entry| !entry.is_empty())
                .map(PathBuf::from)
                .collect::<Vec<_>>()
        })
        .filter(|roots| !roots.is_empty())
        .or_else(|| {
            crate::env_lock::var_os("HOME").map(|home| vec![PathBuf::from(home).join(".codex")])
        })
        .unwrap_or_default();
    configured
        .into_iter()
        .flat_map(expand_rollout_root)
        .collect()
}

fn expand_rollout_root(root: PathBuf) -> Vec<PathBuf> {
    if root.extension().and_then(|value| value.to_str()) == Some("jsonl") {
        return vec![root];
    }
    let sessions = root.join("sessions");
    let archived = root.join("archived_sessions");
    if sessions.is_dir() || archived.is_dir() {
        [sessions, archived]
            .into_iter()
            .filter(|path| path.is_dir())
            .collect()
    } else {
        vec![root]
    }
}

struct Tail {
    pos: u64,
    line: Vec<u8>,
    discarding_oversize_line: bool,
    decoder: Decoder,
}

impl Tail {
    fn bootstrap(path: &Path) -> Self {
        let mut tail = Self {
            pos: 0,
            line: Vec::new(),
            discarding_oversize_line: false,
            decoder: Decoder::default(),
        };
        let Ok(metadata) = std::fs::metadata(path) else {
            return tail;
        };
        let len = metadata.len();

        if let Ok(file) = File::open(path) {
            let mut head = Vec::new();
            let _ = BufReader::new(file)
                .take(BOOTSTRAP_HEAD_BYTES)
                .read_until(b'\n', &mut head);
            if head.last() == Some(&b'\n') {
                head.pop();
            }
            let _ = tail.decoder.process_line(&head);
        }

        if let Ok(mut file) = File::open(path) {
            let start = len.saturating_sub(BOOTSTRAP_TAIL_BYTES);
            if file.seek(SeekFrom::Start(start)).is_ok() {
                let mut bytes = Vec::new();
                let _ = file.take(BOOTSTRAP_TAIL_BYTES).read_to_end(&mut bytes);
                if start > 0 {
                    if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
                        bytes.drain(..=newline);
                    } else {
                        bytes.clear();
                    }
                }
                let ends_with_newline = bytes.last() == Some(&b'\n');
                let mut lines = bytes.split(|byte| *byte == b'\n').peekable();
                while let Some(line) = lines.next() {
                    if lines.peek().is_none() && !ends_with_newline {
                        tail.line.extend_from_slice(line);
                        break;
                    }
                    let _ = tail.decoder.process_line(line);
                }
            }
        }
        tail.pos = len;
        tail.discarding_oversize_line = false;
        tail
    }

    fn drain(&mut self, path: &Path) -> Vec<DecodedEvent> {
        let Ok(metadata) = std::fs::metadata(path) else {
            return Vec::new();
        };
        if metadata.len() < self.pos {
            *self = Self::bootstrap(path);
            return Vec::new();
        }
        let available = metadata.len().saturating_sub(self.pos);
        if available == 0 {
            return Vec::new();
        }
        let read_len = available.min(MAX_READ_BYTES);
        let Ok(mut file) = OpenOptions::new().read(true).open(path) else {
            return Vec::new();
        };
        if file.seek(SeekFrom::Start(self.pos)).is_err() {
            return Vec::new();
        }
        let mut bytes = vec![0; read_len as usize];
        let Ok(read) = file.read(&mut bytes) else {
            return Vec::new();
        };
        self.pos += read as u64;

        let mut events = Vec::new();
        for byte in &bytes[..read] {
            if self.discarding_oversize_line {
                if *byte == b'\n' {
                    self.discarding_oversize_line = false;
                }
                continue;
            }
            if *byte == b'\n' {
                if let Some(event) = self.decoder.process_line(&self.line) {
                    events.push(event);
                }
                self.line.clear();
                continue;
            }
            self.line.push(*byte);
            if self.line.len() > MAX_LINE_BYTES {
                self.line.clear();
                self.discarding_oversize_line = true;
            }
        }
        events
    }
}

pub(super) struct TailSet {
    tails: HashMap<PathBuf, Tail>,
}

impl TailSet {
    pub(super) fn new() -> Self {
        Self {
            tails: HashMap::new(),
        }
    }

    pub(super) fn rescan(&mut self, roots: &[PathBuf], app: &AppHandle) -> usize {
        let active = active_jsonls(roots, SystemTime::now());
        let active_paths: HashSet<_> = active.iter().map(|(path, _, _)| path.clone()).collect();
        self.tails
            .retain(|path, _| active_paths.contains(path) || path.is_file());
        for (path, _, _) in &active {
            if self.tails.contains_key(path) {
                continue;
            }
            let tail = Tail::bootstrap(path);
            if let (Some(thread_id), Some(model_id), Some(observed_at_ms)) = (
                tail.decoder.thread_id.clone(),
                tail.decoder.current_model.clone(),
                tail.decoder.model_observed_at_ms,
            ) {
                let activity = ModelActivity {
                    provider: ProviderId::Codex,
                    model_id,
                    session_or_thread_id: thread_id,
                    observed_at_ms,
                    sequence: tail.decoder.sequence.wrapping_add(1),
                };
                emit_model_activity(app, activity);
            }
            self.tails.insert(path.clone(), tail);
        }
        active.len()
    }

    pub(super) fn pump(&mut self, app: &AppHandle) {
        for (path, tail) in &mut self.tails {
            for event in tail.drain(path) {
                match event {
                    DecodedEvent::Rate(rate) => {
                        let _ = app.emit("burn-tick", rate);
                    }
                    DecodedEvent::Model(activity) => {
                        emit_model_activity(app, activity);
                    }
                }
            }
        }
    }
}

fn active_jsonls(roots: &[PathBuf], now: SystemTime) -> Vec<(PathBuf, SystemTime, u64)> {
    let mut files = Vec::new();
    for root in roots {
        collect_jsonls(root, 0, now, &mut files);
    }
    files.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    files.truncate(MAX_ACTIVE_FILES);
    files
}

fn collect_jsonls(
    path: &Path,
    depth: usize,
    now: SystemTime,
    files: &mut Vec<(PathBuf, SystemTime, u64)>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    let Ok(metadata) = std::fs::symlink_metadata(path) else {
        return;
    };
    if metadata.file_type().is_symlink() {
        return;
    }
    if metadata.is_file() {
        if path.extension().and_then(|value| value.to_str()) != Some("jsonl") {
            return;
        }
        let Ok(modified) = metadata.modified() else {
            return;
        };
        let fresh = now
            .duration_since(modified)
            .map(|age| age <= ACTIVE_WINDOW)
            .unwrap_or(true);
        if fresh {
            files.push((path.to_path_buf(), modified, metadata.len()));
        }
        return;
    }
    if !metadata.is_dir() {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        collect_jsonls(&entry.path(), depth + 1, now, files);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn line(timestamp: &str, kind: &str, payload: &str) -> String {
        format!(r#"{{"timestamp":"{timestamp}","type":"{kind}","payload":{payload}}}"#)
    }

    #[test]
    fn decoder_emits_model_thread_and_per_response_rate() {
        let mut decoder = Decoder::default();
        assert!(decoder
            .process_line(
                line(
                    "2026-07-19T10:00:00.000Z",
                    "session_meta",
                    r#"{"id":"thread-1"}"#
                )
                .as_bytes()
            )
            .is_none());
        assert!(decoder
            .process_line(
                line(
                    "2026-07-19T10:00:01.000Z",
                    "event_msg",
                    r#"{"type":"task_started","turn_id":"turn-1","started_at":1784455201}"#
                )
                .as_bytes()
            )
            .is_none());
        let model = decoder
            .process_line(
                line(
                    "2026-07-19T10:00:01.100Z",
                    "turn_context",
                    r#"{"turn_id":"turn-1","model":"gpt-5.6-sol"}"#,
                )
                .as_bytes(),
            )
            .expect("model activity");
        assert_eq!(
            model,
            DecodedEvent::Model(ModelActivity {
                provider: ProviderId::Codex,
                model_id: "gpt-5.6-sol".into(),
                session_or_thread_id: "thread-1".into(),
                observed_at_ms: 1_784_455_201_100,
                sequence: 1,
            })
        );

        let rate = decoder
            .process_line(line("2026-07-19T10:00:03.000Z", "event_msg", r#"{"type":"token_count","info":{"last_token_usage":{"output_tokens":50},"total_token_usage":{"output_tokens":50}}}"#).as_bytes())
            .expect("rate");
        match rate {
            DecodedEvent::Rate(rate) => {
                assert_eq!(rate.session_or_thread_id, "thread-1");
                assert_eq!(rate.output_tokens, 50);
                assert_eq!(rate.elapsed_ms, 2_000);
                assert!((rate.tokens_per_second - 25.0).abs() < f64::EPSILON);
                assert!(!rate.partial);
            }
            _ => panic!("expected rate"),
        }
    }

    #[test]
    fn decoder_ignores_null_malformed_and_duplicate_counts() {
        let mut decoder = Decoder::default();
        assert!(decoder.process_line(b"not-json").is_none());
        assert!(decoder
            .process_line(
                line(
                    "2026-07-19T10:00:00.000Z",
                    "session_meta",
                    r#"{"id":"thread-1"}"#
                )
                .as_bytes()
            )
            .is_none());
        assert!(decoder
            .process_line(
                line(
                    "2026-07-19T10:00:00.000Z",
                    "event_msg",
                    r#"{"type":"task_started","turn_id":"turn-1"}"#
                )
                .as_bytes()
            )
            .is_none());
        assert!(decoder
            .process_line(
                line(
                    "2026-07-19T10:00:01.000Z",
                    "event_msg",
                    r#"{"type":"token_count","info":null}"#
                )
                .as_bytes()
            )
            .is_none());
        let valid = line(
            "2026-07-19T10:00:02.000Z",
            "event_msg",
            r#"{"type":"token_count","info":{"last_token_usage":{"output_tokens":20},"total_token_usage":{"output_tokens":20}}}"#,
        );
        assert!(decoder.process_line(valid.as_bytes()).is_some());
        assert!(decoder.process_line(valid.as_bytes()).is_none());
    }

    #[test]
    fn discovery_is_recursive_and_does_not_follow_symlinks() {
        let root = std::env::temp_dir().join(format!(
            "cc-autobahn-codex-discovery-{}-{}",
            std::process::id(),
            crate::providers::now_epoch_ms()
        ));
        let nested = root.join("2026/07/19");
        std::fs::create_dir_all(&nested).unwrap();
        let rollout = nested.join("rollout.jsonl");
        File::create(&rollout).unwrap();
        let found = active_jsonls(std::slice::from_ref(&root), SystemTime::now());
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].0, rollout);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn bootstrap_hydrates_state_without_replaying_old_rate() {
        let path = std::env::temp_dir().join(format!(
            "cc-autobahn-codex-tail-{}-{}.jsonl",
            std::process::id(),
            crate::providers::now_epoch_ms()
        ));
        {
            let mut file = File::create(&path).unwrap();
            writeln!(
                file,
                "{}",
                line(
                    "2026-07-19T10:00:00.000Z",
                    "session_meta",
                    r#"{"id":"thread-1"}"#
                )
            )
            .unwrap();
            writeln!(
                file,
                "{}",
                line(
                    "2026-07-19T10:00:01.000Z",
                    "event_msg",
                    r#"{"type":"task_started","turn_id":"turn-1"}"#
                )
            )
            .unwrap();
            writeln!(file, "{}", line("2026-07-19T10:00:02.000Z", "event_msg", r#"{"type":"token_count","info":{"last_token_usage":{"output_tokens":10},"total_token_usage":{"output_tokens":10}}}"#)).unwrap();
        }
        let mut tail = Tail::bootstrap(&path);
        assert_eq!(tail.decoder.thread_id.as_deref(), Some("thread-1"));
        assert!(tail.drain(&path).is_empty());

        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(file, "{}", line("2026-07-19T10:00:04.000Z", "event_msg", r#"{"type":"token_count","info":{"last_token_usage":{"output_tokens":20},"total_token_usage":{"output_tokens":30}}}"#)).unwrap();
        }
        let events = tail.drain(&path);
        assert_eq!(events.len(), 1);
        match &events[0] {
            DecodedEvent::Rate(rate) => {
                assert_eq!(rate.output_tokens, 20);
                assert_eq!(rate.elapsed_ms, 2_000);
            }
            _ => panic!("expected rate"),
        }
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn real_recent_rollout_bootstraps_identity_when_available() {
        let roots = discover_rollout_roots();
        let files = active_jsonls(&roots, SystemTime::now());
        let Some(tail) = files.iter().find_map(|(path, _, _)| {
            let tail = Tail::bootstrap(path);
            let compatible = tail
                .decoder
                .thread_id
                .as_deref()
                .is_some_and(|id| !id.is_empty())
                && tail
                    .decoder
                    .current_model
                    .as_deref()
                    .is_some_and(|model| !model.is_empty());
            compatible.then_some(tail)
        }) else {
            return;
        };
        assert!(
            tail.decoder
                .thread_id
                .as_deref()
                .is_some_and(|id| !id.is_empty()),
            "recent rollout should expose session_meta.id"
        );
        assert!(
            tail.decoder
                .current_model
                .as_deref()
                .is_some_and(|model| !model.is_empty()),
            "recent rollout should expose a bounded turn_context.model"
        );
    }
}
