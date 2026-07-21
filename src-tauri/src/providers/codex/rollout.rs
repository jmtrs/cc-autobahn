//! Defensive discovery and tailing for Codex rollout JSONL.

use std::collections::{HashMap, HashSet};
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use serde::Serialize;
use serde_json::Value;
use tauri::{AppHandle, Emitter, Manager};

use super::scanner::extract_string_property;
use crate::burn::zulu::parse_zulu_millis;
use crate::providers::{emit_model_activity, ModelActivity, ProviderId, SourceQuality, TurnRate};

const ACTIVE_WINDOW: Duration = Duration::from_secs(60 * 60);
const MAX_DEPTH: usize = 8;
const MAX_ACTIVE_FILES: usize = 512;
const MAX_DORMANT_CHECKPOINTS: usize = 512;
const MAX_DORMANT_BYTES: usize = 8 * 1024 * 1024;
const MAX_IDENTIFIER_BYTES: usize = 1024;
const MAX_PENDING_DESKTOP_PERMISSIONS: usize = 64;
const DESKTOP_PERMISSION_TTL_MS: i64 = 120_000;
const MAX_PERMISSION_SUMMARY_BYTES: usize = 1024;
const MAX_READ_BYTES: u64 = 1024 * 1024;
const MAX_LINE_BYTES: usize = 1024 * 1024;
const BOOTSTRAP_TAIL_BYTES: u64 = 1024 * 1024;
const BOOTSTRAP_HEAD_BYTES: u64 = 256 * 1024;

#[derive(Debug, Clone, PartialEq)]
enum DecodedEvent {
    Rate(TurnRate),
    Model(ModelActivity),
    DesktopPermissionPending(DesktopPermissionNotice),
    DesktopPermissionResolved(DesktopPermissionResolution),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopPermissionNotice {
    id: String,
    provider: ProviderId,
    tool_name: String,
    tool_input_summary: String,
    cwd: String,
    observed_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
struct DesktopPermissionResolution {
    id: String,
}

#[derive(Clone, Default)]
struct Decoder {
    thread_id: Option<String>,
    session_started_at_ms: Option<i64>,
    turn_id: Option<String>,
    current_model: Option<String>,
    cwd: Option<String>,
    originator: Option<String>,
    pending_desktop_permissions: HashMap<String, DesktopPermissionNotice>,
    model_observed_at_ms: Option<i64>,
    turn_started_at_ms: Option<i64>,
    last_response_at_ms: Option<i64>,
    last_total_output: Option<u64>,
    sequence: u64,
}

impl Decoder {
    fn allocated_bytes(&self) -> usize {
        let fields: usize = [
            &self.thread_id,
            &self.turn_id,
            &self.current_model,
            &self.cwd,
            &self.originator,
        ]
        .into_iter()
        .flatten()
        .map(String::capacity)
        .sum();
        fields
            + self
                .pending_desktop_permissions
                .iter()
                .map(|(id, notice)| {
                    id.capacity()
                        + notice.tool_name.capacity()
                        + notice.tool_input_summary.capacity()
                        + notice.cwd.capacity()
                })
                .sum::<usize>()
    }

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
                self.session_started_at_ms = observed_at_ms;
                self.originator = nonempty_string(payload.get("originator"));
                None
            }
            "turn_context" => {
                if let Some(cwd) = nonempty_string(payload.get("cwd")) {
                    self.cwd = Some(cwd);
                }
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
            "response_item" => self.decode_response_item(payload, observed_at_ms),
            _ => None,
        }
    }

    fn decode_response_item(
        &mut self,
        payload: &Value,
        observed_at_ms: Option<i64>,
    ) -> Option<DecodedEvent> {
        if self.originator.as_deref() != Some("Codex Desktop") {
            return None;
        }
        match payload.get("type").and_then(Value::as_str) {
            Some("custom_tool_call")
                if payload.get("name").and_then(Value::as_str) == Some("exec") =>
            {
                let input = payload.get("input")?.as_str()?;
                if extract_string_property(input, "sandbox_permissions").as_deref()
                    != Some("require_escalated")
                {
                    return None;
                }
                let id = nonempty_string(payload.get("call_id"))?;
                let observed_at_ms = observed_at_ms?;
                self.purge_expired_desktop_permissions(observed_at_ms);
                if self.pending_desktop_permissions.contains_key(&id)
                    || self.pending_desktop_permissions.len() >= MAX_PENDING_DESKTOP_PERMISSIONS
                {
                    return None;
                }
                let summary = extract_string_property(input, "cmd")
                    .or_else(|| extract_string_property(input, "justification"))
                    .map(|value| truncate_utf8(value, MAX_PERMISSION_SUMMARY_BYTES))
                    .unwrap_or_else(|| "Elevated command requested".to_string());
                let notice = DesktopPermissionNotice {
                    id: id.clone(),
                    provider: ProviderId::Codex,
                    tool_name: "Command".to_string(),
                    tool_input_summary: summary,
                    cwd: self.cwd.clone().unwrap_or_default(),
                    observed_at_ms,
                };
                self.pending_desktop_permissions.insert(id, notice.clone());
                Some(DecodedEvent::DesktopPermissionPending(notice))
            }
            Some("custom_tool_call_output") => {
                let id = nonempty_string(payload.get("call_id"))?;
                self.pending_desktop_permissions.remove(&id).map(|_| {
                    DecodedEvent::DesktopPermissionResolved(DesktopPermissionResolution { id })
                })
            }
            Some("function_call")
                if payload.get("name").and_then(Value::as_str) == Some("exec_command") =>
            {
                let arguments: Value =
                    serde_json::from_str(payload.get("arguments")?.as_str()?).ok()?;
                if arguments.get("sandbox_permissions").and_then(Value::as_str)
                    != Some("require_escalated")
                {
                    return None;
                }
                let id = nonempty_string(payload.get("call_id"))?;
                let observed_at_ms = observed_at_ms?;
                self.purge_expired_desktop_permissions(observed_at_ms);
                if self.pending_desktop_permissions.contains_key(&id)
                    || self.pending_desktop_permissions.len() >= MAX_PENDING_DESKTOP_PERMISSIONS
                {
                    return None;
                }
                let summary = arguments
                    .get("cmd")
                    .and_then(Value::as_str)
                    .map(|value| truncate_utf8(value.to_string(), MAX_PERMISSION_SUMMARY_BYTES))
                    .unwrap_or_else(|| "Elevated command requested".to_string());
                let notice = DesktopPermissionNotice {
                    id: id.clone(),
                    provider: ProviderId::Codex,
                    tool_name: "Command".to_string(),
                    tool_input_summary: summary,
                    cwd: self.cwd.clone().unwrap_or_default(),
                    observed_at_ms,
                };
                self.pending_desktop_permissions.insert(id, notice.clone());
                Some(DecodedEvent::DesktopPermissionPending(notice))
            }
            Some("function_call_output") => {
                let id = nonempty_string(payload.get("call_id"))?;
                self.pending_desktop_permissions.remove(&id).map(|_| {
                    DecodedEvent::DesktopPermissionResolved(DesktopPermissionResolution { id })
                })
            }
            _ => None,
        }
    }

    fn purge_expired_desktop_permissions(&mut self, now_ms: i64) {
        self.pending_desktop_permissions.retain(|_, notice| {
            now_ms.saturating_sub(notice.observed_at_ms) <= DESKTOP_PERMISSION_TTL_MS
        });
    }

    fn decode_token_count(&mut self, payload: &Value, observed_at_ms: i64) -> Option<DecodedEvent> {
        let info = payload.get("info")?.as_object()?;
        let last_usage = info.get("last_token_usage")?;
        let output_tokens = last_usage.get("output_tokens")?.as_u64()?;
        let total_output = info
            .get("total_token_usage")
            .and_then(|usage| usage.get("output_tokens"))
            .and_then(Value::as_u64);
        // Context window fill + cache hit rate, both from the SAME `last_token_usage`
        // (current turn, not the session-wide `total_token_usage` cumulative counter —
        // that one keeps growing past the window size and isn't what's "left").
        let context_used_pct = last_usage
            .get("total_tokens")
            .and_then(Value::as_u64)
            .zip(info.get("model_context_window").and_then(Value::as_u64))
            .filter(|(_, window)| *window > 0)
            .map(|(used, window)| (used as f64 / window as f64 * 100.0).min(100.0));
        let cache_hit_pct = last_usage
            .get("cached_input_tokens")
            .and_then(Value::as_u64)
            .zip(last_usage.get("input_tokens").and_then(Value::as_u64))
            .filter(|(_, input)| *input > 0)
            .map(|(cached, input)| (cached as f64 / input as f64 * 100.0).min(100.0));

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
            source_quality: SourceQuality::Local,
            session_or_thread_id: self.thread_id.clone()?,
            session_started_at_ms: self.session_started_at_ms,
            observed_at_ms,
            output_tokens,
            elapsed_ms,
            tokens_per_second,
            partial: false,
            context_used_pct,
            cache_hit_pct,
        }))
    }
}

fn nonempty_string(value: Option<&Value>) -> Option<String> {
    let value = value?.as_str()?.trim();
    (!value.is_empty() && value.len() <= MAX_IDENTIFIER_BYTES).then(|| value.to_string())
}

fn epoch_seconds_ms(value: Option<&Value>) -> Option<i64> {
    let seconds = value?.as_i64()?;
    seconds.checked_mul(1000)
}

fn truncate_utf8(mut value: String, max_bytes: usize) -> String {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value.truncate(end);
    value.push('…');
    value
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

#[derive(Clone)]
struct TailCheckpoint {
    pos: u64,
    line: Vec<u8>,
    discarding_oversize_line: bool,
    decoder: Decoder,
}

impl TailCheckpoint {
    fn allocated_bytes(&self) -> usize {
        self.line.capacity() + self.decoder.allocated_bytes()
    }
}

impl Tail {
    fn from_checkpoint(checkpoint: TailCheckpoint) -> Self {
        Self {
            pos: checkpoint.pos,
            line: checkpoint.line,
            discarding_oversize_line: checkpoint.discarding_oversize_line,
            decoder: checkpoint.decoder,
        }
    }

    fn checkpoint(&self) -> TailCheckpoint {
        TailCheckpoint {
            pos: self.pos,
            line: self.line.clone(),
            discarding_oversize_line: self.discarding_oversize_line,
            decoder: self.decoder.clone(),
        }
    }

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
    dormant: HashMap<PathBuf, (TailCheckpoint, SystemTime)>,
}

pub(crate) type DesktopPermissionState = Arc<Mutex<HashMap<String, DesktopPermissionNotice>>>;

pub(crate) fn new_desktop_permission_state() -> DesktopPermissionState {
    Arc::new(Mutex::new(HashMap::new()))
}

pub(crate) fn desktop_permission_snapshot(
    state: &DesktopPermissionState,
) -> Vec<DesktopPermissionNotice> {
    let now_ms = crate::providers::now_epoch_ms();
    let mut state = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    state.retain(|_, notice| {
        now_ms.saturating_sub(notice.observed_at_ms) <= DESKTOP_PERMISSION_TTL_MS
    });
    state.values().cloned().collect()
}

fn publish_desktop_permission(app: &AppHandle, notice: DesktopPermissionNotice) {
    let Some(state) = app.try_state::<DesktopPermissionState>() else {
        return;
    };
    let is_new = state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .insert(notice.id.clone(), notice.clone())
        .is_none();
    if is_new {
        crate::window::show_for_permission(app);
        let _ = app.emit("desktop-permission-pending", notice);
    }
}

fn resolve_desktop_permission(app: &AppHandle, resolution: DesktopPermissionResolution) {
    if let Some(state) = app.try_state::<DesktopPermissionState>() {
        state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&resolution.id);
    }
    let _ = app.emit("desktop-permission-resolved", resolution);
    // Only auto-close once NOTHING permission-related is pending — the hook
    // FIFO queue (permission::mod.rs) is a separate source from this map.
    let desktop_pending = app
        .try_state::<DesktopPermissionState>()
        .map(|state| !desktop_permission_snapshot(&state).is_empty())
        .unwrap_or(false);
    if !desktop_pending && !crate::permission::has_pending(app) {
        crate::window::maybe_close_after_permission(app);
    }
}

impl TailSet {
    pub(super) fn new() -> Self {
        Self {
            tails: HashMap::new(),
            dormant: HashMap::new(),
        }
    }

    pub(super) fn rescan(&mut self, roots: &[PathBuf], app: &AppHandle) -> usize {
        let active = active_jsonls(roots, SystemTime::now());
        let active_paths: HashSet<_> = active.iter().map(|(path, _, _)| path.clone()).collect();
        let inactive: Vec<_> = self
            .tails
            .keys()
            .filter(|path| !active_paths.contains(*path))
            .cloned()
            .collect();
        for path in inactive {
            if let Some(tail) = self.tails.remove(&path) {
                self.dormant
                    .insert(path, (tail.checkpoint(), SystemTime::now()));
            }
        }
        self.trim_dormant();

        for (path, _, _) in &active {
            if self.tails.contains_key(path) {
                continue;
            }
            let tail = self
                .dormant
                .remove(path)
                .map(|(checkpoint, _)| Tail::from_checkpoint(checkpoint))
                .unwrap_or_else(|| Tail::bootstrap(path));
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
            for notice in tail.decoder.pending_desktop_permissions.values() {
                if crate::providers::now_epoch_ms().saturating_sub(notice.observed_at_ms)
                    <= DESKTOP_PERMISSION_TTL_MS
                {
                    publish_desktop_permission(app, notice.clone());
                }
            }
            self.tails.insert(path.clone(), tail);
        }
        active.len()
    }

    fn trim_dormant(&mut self) {
        let mut dormant_bytes: usize = self
            .dormant
            .values()
            .map(|(checkpoint, _)| checkpoint.allocated_bytes())
            .sum();
        if self.dormant.len() <= MAX_DORMANT_CHECKPOINTS && dormant_bytes <= MAX_DORMANT_BYTES {
            return;
        }
        let mut oldest: Vec<_> = self
            .dormant
            .iter()
            .map(|(path, (_, seen_at))| (path.clone(), *seen_at))
            .collect();
        oldest.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
        for (path, _) in oldest {
            if self.dormant.len() <= MAX_DORMANT_CHECKPOINTS && dormant_bytes <= MAX_DORMANT_BYTES {
                break;
            }
            if let Some((checkpoint, _)) = self.dormant.remove(&path) {
                dormant_bytes = dormant_bytes.saturating_sub(checkpoint.allocated_bytes());
            }
        }
    }

    pub(super) fn pump(&mut self, app: &AppHandle) {
        if let Some(state) = app.try_state::<DesktopPermissionState>() {
            let now_ms = crate::providers::now_epoch_ms();
            state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .retain(|_, notice| {
                    now_ms.saturating_sub(notice.observed_at_ms) <= DESKTOP_PERMISSION_TTL_MS
                });
        }
        for (path, tail) in &mut self.tails {
            let events = tail.drain(path);
            let resolved_in_batch: HashSet<_> = events
                .iter()
                .filter_map(|event| match event {
                    DecodedEvent::DesktopPermissionResolved(resolution) => {
                        Some(resolution.id.clone())
                    }
                    _ => None,
                })
                .collect();
            for event in events {
                match event {
                    DecodedEvent::Rate(rate) => {
                        let _ = app.emit("burn-tick", rate);
                    }
                    DecodedEvent::Model(activity) => {
                        emit_model_activity(app, activity);
                    }
                    DecodedEvent::DesktopPermissionPending(notice) => {
                        if !resolved_in_batch.contains(&notice.id) {
                            publish_desktop_permission(app, notice);
                        }
                    }
                    DecodedEvent::DesktopPermissionResolved(resolution) => {
                        resolve_desktop_permission(app, resolution);
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
                assert_eq!(rate.source_quality, SourceQuality::Local);
                assert_eq!(rate.session_or_thread_id, "thread-1");
                assert_eq!(rate.session_started_at_ms, Some(1_784_455_200_000));
                assert_eq!(rate.output_tokens, 50);
                assert_eq!(rate.elapsed_ms, 2_000);
                assert!((rate.tokens_per_second - 25.0).abs() < f64::EPSILON);
                assert!(!rate.partial);
            }
            _ => panic!("expected rate"),
        }
    }

    #[test]
    fn concurrent_root_and_subagent_rollouts_keep_decoder_identity_isolated() {
        let mut root = Decoder::default();
        let mut subagent = Decoder::default();
        for (decoder, thread_id) in [
            (&mut root, "thread-root"),
            (&mut subagent, "thread-subagent"),
        ] {
            assert!(decoder
                .process_line(
                    line(
                        "2026-07-19T10:00:00.000Z",
                        "session_meta",
                        &format!(r#"{{"id":"{thread_id}"}}"#),
                    )
                    .as_bytes(),
                )
                .is_none());
        }

        let root_event = root
            .process_line(
                line(
                    "2026-07-19T10:00:01.000Z",
                    "turn_context",
                    r#"{"model":"gpt-5.6-sol"}"#,
                )
                .as_bytes(),
            )
            .expect("root model");
        let subagent_event = subagent
            .process_line(
                line(
                    "2026-07-19T10:00:01.100Z",
                    "turn_context",
                    r#"{"model":"gpt-5.6-terra"}"#,
                )
                .as_bytes(),
            )
            .expect("subagent model");

        let DecodedEvent::Model(root_model) = root_event else {
            panic!("expected root model activity");
        };
        let DecodedEvent::Model(subagent_model) = subagent_event else {
            panic!("expected subagent model activity");
        };
        assert_eq!(root_model.session_or_thread_id, "thread-root");
        assert_eq!(subagent_model.session_or_thread_id, "thread-subagent");
        assert_eq!(root_model.model_id, "gpt-5.6-sol");
        assert_eq!(subagent_model.model_id, "gpt-5.6-terra");
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
    fn decoder_emits_desktop_permission_lifecycle_for_escalated_exec() {
        let mut decoder = Decoder::default();
        assert!(decoder
            .process_line(
                line(
                    "2026-07-20T07:22:38.000Z",
                    "session_meta",
                    r#"{"id":"thread-1","originator":"Codex Desktop"}"#,
                )
                .as_bytes(),
            )
            .is_none());
        let _ = decoder.process_line(
            line(
                "2026-07-20T07:22:39.000Z",
                "turn_context",
                r#"{"model":"gpt-5.6-sol","cwd":"/tmp/project"}"#,
            )
            .as_bytes(),
        );
        let input = r#"const result = await tools.exec_command({cmd:"touch /tmp/permission-test", sandbox_permissions: "require_escalated", justification: "Allow this permission test?"}); text(result.output)"#;
        let pending_payload = serde_json::json!({
            "type": "custom_tool_call",
            "name": "exec",
            "call_id": "call-permission-1",
            "input": input,
        });
        let pending = decoder
            .process_line(
                line(
                    "2026-07-20T07:22:40.246Z",
                    "response_item",
                    &pending_payload.to_string(),
                )
                .as_bytes(),
            )
            .expect("pending desktop permission");
        assert_eq!(
            pending,
            DecodedEvent::DesktopPermissionPending(DesktopPermissionNotice {
                id: "call-permission-1".into(),
                provider: ProviderId::Codex,
                tool_name: "Command".into(),
                tool_input_summary: "touch /tmp/permission-test".into(),
                cwd: "/tmp/project".into(),
                observed_at_ms: 1_784_532_160_246,
            })
        );
        assert!(decoder
            .process_line(
                line(
                    "2026-07-20T07:22:41.000Z",
                    "response_item",
                    &pending_payload.to_string(),
                )
                .as_bytes(),
            )
            .is_none());

        let resolved = decoder
            .process_line(
                line(
                    "2026-07-20T07:23:32.807Z",
                    "response_item",
                    r#"{"type":"custom_tool_call_output","call_id":"call-permission-1","output":"ok"}"#,
                )
                .as_bytes(),
            )
            .expect("resolved desktop permission");
        assert_eq!(
            resolved,
            DecodedEvent::DesktopPermissionResolved(DesktopPermissionResolution {
                id: "call-permission-1".into()
            })
        );
    }

    #[test]
    fn decoder_ignores_non_escalated_and_unmatched_desktop_calls() {
        let mut decoder = Decoder::default();
        assert!(decoder
            .process_line(
                line(
                    "2026-07-20T07:22:38.000Z",
                    "session_meta",
                    r#"{"id":"thread-1","originator":"Codex Desktop"}"#,
                )
                .as_bytes(),
            )
            .is_none());
        for input in [
            r#"text(await tools.exec_command({cmd:"pwd"}))"#,
            r#"text(await tools.exec_command({cmd:"pwd", sandbox_permissions: "use_default"}))"#,
        ] {
            let payload = serde_json::json!({
                "type": "custom_tool_call",
                "name": "exec",
                "call_id": format!("call-{input}"),
                "input": input,
            });
            assert!(decoder
                .process_line(
                    line(
                        "2026-07-20T07:22:40.246Z",
                        "response_item",
                        &payload.to_string(),
                    )
                    .as_bytes(),
                )
                .is_none());
        }
        assert!(decoder
            .process_line(
                line(
                    "2026-07-20T07:23:32.807Z",
                    "response_item",
                    r#"{"type":"custom_tool_call_output","call_id":"unknown"}"#,
                )
                .as_bytes(),
            )
            .is_none());
    }

    #[test]
    fn decoder_supports_legacy_desktop_function_call_format_only_for_desktop() {
        let escalation = serde_json::json!({
            "type": "function_call",
            "name": "exec_command",
            "call_id": "legacy-call",
            "arguments": r#"{"cmd":"whoami","sandbox_permissions":"require_escalated"}"#,
        });
        let mut desktop = Decoder::default();
        let _ = desktop.process_line(
            line(
                "2026-07-20T07:22:38.000Z",
                "session_meta",
                r#"{"id":"thread-1","originator":"Codex Desktop"}"#,
            )
            .as_bytes(),
        );
        assert!(matches!(
            desktop.process_line(
                line(
                    "2026-07-20T07:22:40.000Z",
                    "response_item",
                    &escalation.to_string(),
                )
                .as_bytes(),
            ),
            Some(DecodedEvent::DesktopPermissionPending(_))
        ));
        assert!(matches!(
            desktop.process_line(
                line(
                    "2026-07-20T07:22:41.000Z",
                    "response_item",
                    r#"{"type":"function_call_output","call_id":"legacy-call"}"#,
                )
                .as_bytes(),
            ),
            Some(DecodedEvent::DesktopPermissionResolved(_))
        ));

        let mut cli = Decoder::default();
        let _ = cli.process_line(
            line(
                "2026-07-20T07:22:38.000Z",
                "session_meta",
                r#"{"id":"thread-2","originator":"codex_cli_rs"}"#,
            )
            .as_bytes(),
        );
        assert!(cli
            .process_line(
                line(
                    "2026-07-20T07:22:40.000Z",
                    "response_item",
                    &escalation.to_string(),
                )
                .as_bytes(),
            )
            .is_none());
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
    fn dormant_checkpoint_preserves_first_append_after_reactivation() {
        let path = std::env::temp_dir().join(format!(
            "cc-autobahn-codex-reactivation-{}-{}.jsonl",
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
        }
        let tail = Tail::bootstrap(&path);
        let checkpoint = tail.checkpoint();
        {
            let mut file = OpenOptions::new().append(true).open(&path).unwrap();
            writeln!(file, "{}", line("2026-07-19T10:00:03.000Z", "event_msg", r#"{"type":"token_count","info":{"last_token_usage":{"output_tokens":40},"total_token_usage":{"output_tokens":40}}}"#)).unwrap();
        }
        let mut resumed = Tail::from_checkpoint(checkpoint);
        let events = resumed.drain(&path);
        assert_eq!(events.len(), 1);
        let DecodedEvent::Rate(rate) = &events[0] else {
            panic!("expected resumed rate");
        };
        assert_eq!(rate.output_tokens, 40);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn checkpoint_preserves_partial_line_bytes_and_decoder_state() {
        let decoder = Decoder {
            thread_id: Some("thread-partial".into()),
            ..Decoder::default()
        };
        let tail = Tail {
            pos: 12,
            line: br#"{"timestamp""#.to_vec(),
            discarding_oversize_line: false,
            decoder,
        };
        let resumed = Tail::from_checkpoint(tail.checkpoint());
        assert_eq!(resumed.line, br#"{"timestamp""#);
        assert_eq!(resumed.decoder.thread_id.as_deref(), Some("thread-partial"));
    }

    #[test]
    fn decoder_rejects_oversized_identity_fields() {
        let oversized = Value::String("x".repeat(MAX_IDENTIFIER_BYTES + 1));
        assert!(nonempty_string(Some(&oversized)).is_none());
        let bounded = Value::String("x".repeat(MAX_IDENTIFIER_BYTES));
        assert_eq!(
            nonempty_string(Some(&bounded)).map(|value| value.len()),
            Some(MAX_IDENTIFIER_BYTES)
        );
    }

    #[test]
    fn dormant_checkpoints_obey_the_aggregate_byte_budget() {
        let mut tails = TailSet::new();
        for index in 0..9 {
            tails.dormant.insert(
                PathBuf::from(format!("rollout-{index}.jsonl")),
                (
                    TailCheckpoint {
                        pos: 0,
                        line: vec![b'x'; 1024 * 1024],
                        discarding_oversize_line: false,
                        decoder: Decoder::default(),
                    },
                    SystemTime::UNIX_EPOCH + Duration::from_secs(index),
                ),
            );
        }
        tails.trim_dormant();
        let bytes: usize = tails
            .dormant
            .values()
            .map(|(checkpoint, _)| checkpoint.allocated_bytes())
            .sum();
        assert!(bytes <= MAX_DORMANT_BYTES);
        assert_eq!(tails.dormant.len(), 8);
        assert!(!tails.dormant.contains_key(Path::new("rollout-0.jsonl")));
    }

    #[test]
    fn dormant_budget_includes_decoder_allocations() {
        let mut tails = TailSet::new();
        for index in 0..9 {
            tails.dormant.insert(
                PathBuf::from(format!("decoder-{index}.jsonl")),
                (
                    TailCheckpoint {
                        pos: 0,
                        line: Vec::new(),
                        discarding_oversize_line: false,
                        decoder: Decoder {
                            current_model: Some("x".repeat(1024 * 1024)),
                            ..Decoder::default()
                        },
                    },
                    SystemTime::UNIX_EPOCH + Duration::from_secs(index),
                ),
            );
        }
        tails.trim_dormant();
        let bytes: usize = tails
            .dormant
            .values()
            .map(|(checkpoint, _)| checkpoint.allocated_bytes())
            .sum();
        assert!(bytes <= MAX_DORMANT_BYTES);
        assert_eq!(tails.dormant.len(), 8);
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
