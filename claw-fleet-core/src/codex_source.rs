//! Codex agent source — scans ~/.codex/ for session data.
//!
//! Primary path: read thread metadata from `~/.codex/state_5.sqlite` (fast).
//! Fallback: scan `~/.codex/sessions/YYYY/MM/DD/` for rollout JSONL files.
//!
//! For active sessions (recently updated), the rollout file is still read
//! to determine fine-grained status (streaming, thinking, executing, etc.)
//! and to compute token speed.
//!
//! Codex stores sessions under `~/.codex/sessions/YYYY/MM/DD/`:
//!   - `rollout-<timestamp>-<thread-id>.jsonl.zst` (zstd-compressed JSONL)
//!   - Each line is `{"timestamp":"...","type":"<variant>","payload":{...}}`
//!
//! Rollout line types:
//!   - `session_meta`   — first line: thread id, cwd, model_provider, source, etc.
//!   - `turn_context`   — emitted per turn: model name, policies, etc.
//!   - `response_item`  — conversation items: message, local_shell_call, reasoning, etc.
//!   - `event_msg`      — events: user_message, agent_message, turn_complete, token_count, etc.
//!   - `compacted`      — context compaction marker.

use std::collections::HashSet;
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::agent_source::{AgentSource, WatchStrategy};
use crate::backend::SourceUsageSummary;
use crate::session::{SessionInfo, SessionStatus, compute_context_percent};

/// URI prefix for Codex session identifiers.
const CODEX_URI_PREFIX: &str = "codex://";

pub struct CodexSource {
    /// `None` timestamp = never scanned, force refresh on first read.
    /// Avoids `Instant::now() - 999s` which panics on Windows runners
    /// with low uptime ("overflow when subtracting duration from instant").
    process_cache: std::sync::Mutex<(Option<std::time::Instant>, Vec<CodexProcess>)>,
}

/// A running Codex process with its PID, working directory, and optional thread ID.
#[derive(Clone)]
struct CodexProcess {
    pid: u32,
    cwd: String,
    /// Thread ID extracted from `--thread <id>` or `--resume <id>` command-line args.
    thread_id: Option<String>,
}

/// Thread metadata read from SQLite.
struct SqliteThread {
    id: String,
    rollout_path: String,
    created_at: i64,
    updated_at: i64,
    source: String,
    cwd: String,
    title: String,
    model: Option<String>,
    tokens_used: i64,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
    #[allow(dead_code)]
    archived: bool,
    first_user_message: String,
}


impl CodexSource {
    pub fn new() -> Self {
        Self {
            process_cache: std::sync::Mutex::new((None, Vec::new())),
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn get_codex_dir() -> Option<PathBuf> {
    if let Ok(codex_home) = std::env::var("CODEX_HOME") {
        return Some(PathBuf::from(codex_home));
    }
    crate::session::real_home_dir().map(|h| h.join(".codex"))
}

fn get_sessions_dir() -> Option<PathBuf> {
    get_codex_dir().map(|d| d.join("sessions"))
}

fn get_sqlite_path() -> Option<PathBuf> {
    get_codex_dir().map(|d| d.join("state_5.sqlite"))
}

/// Resolve a `codex://` URI to the actual rollout file path.
/// Format: `codex://<relative-path-under-sessions-dir>`
fn resolve_uri(uri: &str) -> Option<PathBuf> {
    let stripped = uri.strip_prefix(CODEX_URI_PREFIX)?;
    // If the stored path is absolute (from SQLite rollout_path), use it directly.
    let path = PathBuf::from(stripped);
    if path.is_absolute() {
        return Some(path);
    }
    let sessions_dir = get_sessions_dir()?;
    Some(sessions_dir.join(stripped))
}

/// Build a codex:// URI from a rollout file path.
fn build_uri(path: &Path) -> Option<String> {
    let sessions_dir = get_sessions_dir()?;
    if let Ok(rel) = path.strip_prefix(&sessions_dir) {
        Some(format!("{}{}", CODEX_URI_PREFIX, rel.to_string_lossy()))
    } else {
        // Absolute path not under sessions dir — store absolute.
        Some(format!("{}{}", CODEX_URI_PREFIX, path.to_string_lossy()))
    }
}

/// Read and decompress a .jsonl.zst file.
fn read_zst_file(path: &Path) -> Result<String, String> {
    let file =
        fs::File::open(path).map_err(|e| format!("Cannot open {}: {e}", path.display()))?;
    let mut decoder =
        zstd::Decoder::new(file).map_err(|e| format!("zstd decode error: {e}"))?;
    let mut content = String::new();
    decoder
        .read_to_string(&mut content)
        .map_err(|e| format!("zstd read error: {e}"))?;
    Ok(content)
}

/// Read a Codex session file (supports both .jsonl.zst and plain .jsonl).
fn read_session_content(path: &Path) -> Result<String, String> {
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name.ends_with(".jsonl.zst") {
        read_zst_file(path)
    } else {
        crate::bom::read_to_string_no_bom(path)
            .map_err(|e| format!("Cannot read {}: {e}", path.display()))
    }
}

/// Recursively find all rollout files under a directory.
fn find_rollout_files(dir: &Path) -> Vec<PathBuf> {
    let mut result = Vec::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return result;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Skip "archived" directory.
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name == "archived" {
                continue;
            }
            result.extend(find_rollout_files(&path));
        } else {
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default();
            if name.ends_with(".jsonl.zst") || name.ends_with(".jsonl") {
                result.push(path);
            }
        }
    }
    result
}

/// Extract session_meta payload from parsed JSONL lines.
fn extract_session_meta(lines: &[Value]) -> Option<&Value> {
    lines
        .iter()
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("session_meta"))
        .and_then(|v| v.get("payload"))
}

/// Read the rollout `originator` (`session_meta.payload.originator`) from a
/// rollout file, reading **only the first line** — `session_meta` is always
/// line 1, so this stays a few-hundred-byte read even for a huge transcript.
///
/// This is Codex's Fleet-ownership signal: Fleet stamps
/// [`crate::codex_launch::CODEX_FLEET_ORIGINATOR`] via
/// `CODEX_INTERNAL_ORIGINATOR_OVERRIDE`, and it lands here (the SQLite `source`
/// column does not carry it). Surfaced on [`SessionInfo::entrypoint`] so
/// [`crate::session_launch::is_fleet_owned_entrypoint`] classifies the session
/// uniformly with Claude. `None` when the file is unreadable or the line has no
/// originator (e.g. an older rollout format).
fn read_rollout_originator(rollout_path: &Path) -> Option<String> {
    use std::io::{BufRead, BufReader};
    let file = fs::File::open(rollout_path).ok()?;
    let mut first_line = String::new();
    BufReader::new(file).read_line(&mut first_line).ok()?;
    let v: Value = serde_json::from_str(first_line.trim()).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    v.get("payload")
        .and_then(|p| p.get("originator"))
        .and_then(|o| o.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// Read a rollout's first `session_meta` payload (only line 1). Same read shape
/// as [`read_rollout_originator`], but returns the whole payload so callers can
/// pull `id` / `cwd` / `originator` together.
fn read_rollout_meta_payload(rollout_path: &Path) -> Option<Value> {
    use std::io::{BufRead, BufReader};
    let file = fs::File::open(rollout_path).ok()?;
    let mut first_line = String::new();
    BufReader::new(file).read_line(&mut first_line).ok()?;
    let v: Value = serde_json::from_str(first_line.trim()).ok()?;
    if v.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
        return None;
    }
    v.get("payload").cloned()
}

/// SQLite index lookup: a single thread's `(rollout_path, cwd)` by id. `None`
/// when there is no SQLite DB or no matching row.
fn lookup_thread_rollout_cwd(thread_id: &str) -> Option<(String, String)> {
    let db_path = get_sqlite_path()?;
    if !db_path.exists() {
        return None;
    }
    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;
    let mut stmt = conn
        .prepare("SELECT rollout_path, cwd FROM threads WHERE id = ?1 LIMIT 1")
        .ok()?;
    stmt.query_row([thread_id], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })
    .ok()
}

/// For a Fleet-owned Codex session identified by `thread_id`, the workspace it
/// was launched in; `None` if the thread is unknown or not Fleet-owned.
///
/// Codex's analogue of `session_entrypoint` + `resolve_session_cwd` folded into
/// one: those read a `~/.claude` transcript and return `None` for every Codex
/// thread, so the enqueue gate (`pending_message`) and drain belt-and-braces
/// would reject Codex sessions outright (M5 P15). Ownership is the rollout
/// `originator == "fleet"` marker ([`crate::codex_launch::CODEX_FLEET_ORIGINATOR`]);
/// cwd comes from the SQLite row, or the rollout's own `session_meta` when SQLite
/// is unavailable.
pub fn codex_fleet_owned_cwd(thread_id: &str) -> Option<String> {
    let thread_id = thread_id.trim();
    if thread_id.is_empty() {
        return None;
    }
    // Preferred: SQLite maps thread id → rollout path + cwd directly.
    if let Some((rollout_path, cwd)) = lookup_thread_rollout_cwd(thread_id) {
        let originator = read_rollout_originator(Path::new(&rollout_path));
        if crate::session_launch::is_fleet_owned_entrypoint(originator.as_deref()) {
            let cwd = cwd.trim();
            if !cwd.is_empty() {
                return Some(cwd.to_string());
            }
        }
        return None;
    }
    // Fallback (no SQLite): find the rollout whose session_meta id matches and
    // read ownership + cwd from its first line.
    let sessions_dir = get_sessions_dir()?;
    if !sessions_dir.is_dir() {
        return None;
    }
    for path in find_rollout_files(&sessions_dir) {
        let Some(payload) = read_rollout_meta_payload(&path) else {
            continue;
        };
        if payload.get("id").and_then(|v| v.as_str()) != Some(thread_id) {
            continue;
        }
        let originator = payload.get("originator").and_then(|v| v.as_str());
        if !crate::session_launch::is_fleet_owned_entrypoint(originator) {
            return None;
        }
        return payload
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .map(str::to_string);
    }
    None
}

/// Extract the model name from turn_context lines.
fn extract_model(lines: &[Value]) -> Option<String> {
    for line in lines.iter().rev() {
        if line.get("type").and_then(|t| t.as_str()) == Some("turn_context") {
            if let Some(model) = line
                .get("payload")
                .and_then(|p| p.get("model"))
                .and_then(|m| m.as_str())
            {
                if !model.is_empty() {
                    return Some(model.to_string());
                }
            }
        }
    }
    None
}

/// Extract the last text content for preview from the session.
fn extract_last_text(lines: &[Value]) -> Option<String> {
    for line in lines.iter().rev() {
        let line_type = line.get("type").and_then(|t| t.as_str())?;
        let payload = line.get("payload")?;

        match line_type {
            // event_msg with agent_message type
            "event_msg" => {
                let msg_type = payload.get("type").and_then(|t| t.as_str());
                if msg_type == Some("agent_message") {
                    if let Some(text) = payload.get("message").and_then(|m| m.as_str()) {
                        let preview: String = text.chars().take(200).collect();
                        return Some(preview);
                    }
                }
            }
            // response_item with message type and assistant role
            "response_item" => {
                let item_type = payload.get("type").and_then(|t| t.as_str());
                let role = payload.get("role").and_then(|r| r.as_str());
                if item_type == Some("message") && role == Some("assistant") {
                    if let Some(content) = payload.get("content").and_then(|c| c.as_array()) {
                        for block in content.iter().rev() {
                            let block_type = block.get("type").and_then(|t| t.as_str());
                            if block_type == Some("output_text") {
                                if let Some(text) =
                                    block.get("text").and_then(|t| t.as_str())
                                {
                                    let preview: String = text.chars().take(200).collect();
                                    return Some(preview);
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract the *first* user message text from a rollout, forward-scanning.
///
/// Mirrors [`extract_last_text`] but looks for the earliest user turn — used to
/// feed [`crate::codex_title`] on the filesystem fallback path (the SQLite path
/// already carries `first_user_message`).
fn extract_first_user_text(lines: &[Value]) -> Option<String> {
    for line in lines.iter() {
        let Some(line_type) = line.get("type").and_then(|t| t.as_str()) else {
            continue;
        };
        let Some(payload) = line.get("payload") else {
            continue;
        };
        match line_type {
            "event_msg" => {
                if payload.get("type").and_then(|t| t.as_str()) == Some("user_message") {
                    if let Some(text) = payload.get("message").and_then(|m| m.as_str()) {
                        if !text.trim().is_empty() {
                            return Some(text.to_string());
                        }
                    }
                }
            }
            "response_item" => {
                let item_type = payload.get("type").and_then(|t| t.as_str());
                let role = payload.get("role").and_then(|r| r.as_str());
                if item_type == Some("message") && role == Some("user") {
                    if let Some(content) = payload.get("content").and_then(|c| c.as_array()) {
                        for block in content.iter() {
                            let block_type = block.get("type").and_then(|t| t.as_str());
                            if block_type == Some("input_text") || block_type == Some("text") {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    if !text.trim().is_empty() {
                                        return Some(text.to_string());
                                    }
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }
    None
}

/// Determine session status from the last JSONL lines and file age.
fn determine_status(last_lines: &[Value], file_age_secs: f64) -> SessionStatus {
    // Look for the last turn_complete, turn_started, or approval request event.
    let last_turn_event = last_lines.iter().rev().find(|v| {
        v.get("type").and_then(|t| t.as_str()) == Some("event_msg")
            && matches!(
                v.get("payload")
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str()),
                Some("turn_complete")
                    | Some("task_complete")
                    | Some("turn_started")
                    | Some("exec_approval_request")
                    | Some("apply_patch_approval_request")
                    | Some("mcp_approval_request")
            )
    });

    // If file is very fresh, it's likely actively streaming.
    if file_age_secs < 8.0 {
        // Check if the last event is a turn_complete — then it's waiting for input.
        if let Some(evt) = last_turn_event {
            let evt_type = evt
                .get("payload")
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str());
            if matches!(evt_type, Some("turn_complete") | Some("task_complete")) {
                return SessionStatus::WaitingInput;
            }
            // Approval requests mean the agent is waiting for user confirmation.
            if matches!(
                evt_type,
                Some("exec_approval_request")
                    | Some("apply_patch_approval_request")
                    | Some("mcp_approval_request")
            ) {
                return SessionStatus::WaitingInput;
            }
        }

        // Check for shell call or patch apply in progress.
        let has_tool_in_progress = last_lines.iter().rev().take(10).any(|v| {
            let lt = v.get("type").and_then(|t| t.as_str());
            let payload = v.get("payload");
            let item_type = payload
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str());
            let status = payload
                .and_then(|p| p.get("status"))
                .and_then(|s| s.as_str());

            lt == Some("response_item")
                && matches!(
                    item_type,
                    Some("local_shell_call")
                        | Some("function_call")
                        | Some("web_search_call")
                        | Some("image_generation_call")
                )
                && status == Some("in_progress")
        });
        if has_tool_in_progress {
            return SessionStatus::Executing;
        }

        // Check for exec_command_begin without a matching exec_command_end.
        let has_command_running = {
            let mut running = false;
            for v in last_lines.iter().rev().take(20) {
                if v.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
                    continue;
                }
                let msg_type = v
                    .get("payload")
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str());
                match msg_type {
                    Some("exec_command_begin") | Some("patch_apply_begin") => {
                        running = true;
                        break;
                    }
                    Some("exec_command_end") | Some("patch_apply_end") => {
                        break;
                    }
                    _ => {}
                }
            }
            running
        };
        if has_command_running {
            return SessionStatus::Executing;
        }

        // Check for reasoning.
        let has_reasoning = last_lines.iter().rev().take(10).any(|v| {
            v.get("type").and_then(|t| t.as_str()) == Some("response_item")
                && v.get("payload")
                    .and_then(|p| p.get("type"))
                    .and_then(|t| t.as_str())
                    == Some("reasoning")
        });
        if has_reasoning {
            return SessionStatus::Thinking;
        }

        return SessionStatus::Streaming;
    }

    // Check the most recent event.
    if let Some(evt) = last_turn_event {
        let evt_type = evt
            .get("payload")
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str());
        match evt_type {
            // Approval requests — waiting for user regardless of file age.
            Some("exec_approval_request")
            | Some("apply_patch_approval_request")
            | Some("mcp_approval_request") => {
                if file_age_secs < 600.0 {
                    return SessionStatus::WaitingInput;
                }
            }
            Some("turn_complete") | Some("task_complete") if file_age_secs < 300.0 => {
                return SessionStatus::WaitingInput;
            }
            Some("turn_started") if file_age_secs < 120.0 => {
                return SessionStatus::Thinking;
            }
            _ => {}
        }
    }

    if file_age_secs < 30.0 {
        SessionStatus::Active
    } else {
        SessionStatus::Idle
    }
}

/// Compute token speed and total output tokens from parsed JSONL lines.
fn compute_token_stats(lines: &[Value]) -> (f64, u64) {
    let mut total_output: u64 = 0;
    let mut timed_tokens: Vec<(f64, u64)> = Vec::new();

    for line in lines {
        let line_type = line.get("type").and_then(|t| t.as_str()).unwrap_or("");

        // token_count events in event_msg
        if line_type == "event_msg" {
            let payload = line.get("payload");
            let msg_type = payload
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str());

            if msg_type == Some("token_count") {
                if let Some(info) = payload.and_then(|p| p.get("info")) {
                    let cumulative_output = info
                        .get("total_token_usage")
                        .and_then(|u| u.get("output_tokens"))
                        .and_then(|t| t.as_u64())
                        .or_else(|| info.get("output_tokens").and_then(|t| t.as_u64()))
                        .unwrap_or(0);
                    if cumulative_output > 0 {
                        total_output = cumulative_output; // token_count is cumulative
                    }

                    let incremental_output = info
                        .get("last_token_usage")
                        .and_then(|u| u.get("output_tokens"))
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);
                    if incremental_output > 0 {
                        if let Some(ts_str) = line.get("timestamp").and_then(|t| t.as_str()) {
                            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                                timed_tokens.push((dt.timestamp() as f64, incremental_output));
                            }
                        }
                    }
                }
            }

            // Also check turn_complete for usage.
            if matches!(msg_type, Some("turn_complete") | Some("task_complete")) {
                if let Some(usage) = payload.and_then(|p| p.get("usage")) {
                    let output = usage
                        .get("output_tokens")
                        .and_then(|t| t.as_u64())
                        .unwrap_or(0);
                    total_output += output;

                    if let Some(ts_str) = line.get("timestamp").and_then(|t| t.as_str()) {
                        if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                            timed_tokens.push((dt.timestamp() as f64, output));
                        }
                    }
                }
            }
        }
    }

    let speed = if timed_tokens.len() >= 2 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let window_start = now - 300.0;
        let recent: Vec<_> = timed_tokens
            .iter()
            .filter(|(ts, _)| *ts > window_start)
            .collect();
        if recent.len() >= 2 {
            let total_recent: u64 = recent.iter().map(|(_, t)| t).sum();
            let first_ts = recent.first().map(|(ts, _)| *ts).unwrap_or(0.0);
            let last_ts = recent.last().map(|(ts, _)| *ts).unwrap_or(0.0);
            let duration = last_ts - first_ts;
            if duration > 0.0 {
                total_recent as f64 / duration
            } else {
                0.0
            }
        } else {
            0.0
        }
    } else {
        0.0
    };

    (speed, total_output)
}

/// Compute USD cost and cumulative input tokens for a Codex session from its
/// rollout lines, reading the last `token_count` event's cumulative
/// `total_token_usage` (input / cached_input / output).
///
/// Codex's `output_tokens` already includes `reasoning_output_tokens` — verified
/// against real rollouts where `input_tokens + output_tokens == total_tokens` —
/// so reasoning is NOT added separately. `cached_input_tokens` is a subset of
/// `input_tokens`, so full-price input is `input - cached` and the cached portion
/// is billed at the model's cache-read (90%-off) rate.
///
/// Returns `(cost_usd, total_input_tokens)`, or `(0.0, 0)` when no `token_count`
/// with a `total_token_usage` block is present. `model` selects the price tier
/// via `model_cost::get_model_costs`; an absent model falls back to `"gpt"`,
/// which routes to the GPT Sol tier.
fn codex_cost_and_input(lines: &[Value], model: Option<&str>) -> (f64, u64) {
    for line in lines.iter().rev() {
        if line.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
            continue;
        }
        let payload = match line.get("payload") {
            Some(p) => p,
            None => continue,
        };
        if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
            continue;
        }
        let usage = match payload.get("info").and_then(|i| i.get("total_token_usage")) {
            Some(u) => u,
            None => continue,
        };
        let input = usage
            .get("input_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        // cached is a subset of input; clamp defensively so `input - cached`
        // never underflows if a rollout ever reports cached > input.
        let cached = usage
            .get("cached_input_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0)
            .min(input);
        let output = usage
            .get("output_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        if input == 0 && output == 0 {
            continue;
        }
        let turn = crate::model_cost::TurnUsage {
            input_tokens: input - cached,
            output_tokens: output,
            cache_read_tokens: cached,
            cache_creation_tokens: 0,
            web_search_requests: 0,
        };
        let cost = crate::model_cost::turn_cost_usd(model.unwrap_or("gpt"), &turn);
        return (cost, input);
    }
    (0.0, 0)
}

/// Extract context utilization from the latest token_count event.
fn extract_context_percent(lines: &[Value], model: Option<&str>) -> Option<f64> {
    for line in lines.iter().rev() {
        if line.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
            continue;
        }
        let payload = line.get("payload")?;
        if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
            continue;
        }
        let info = payload.get("info")?;
        // `last_token_usage` is the current context-window occupancy for the
        // most recent turn. `total_token_usage` is CUMULATIVE across every turn
        // (Codex resends the whole prompt each turn, so its input grows without
        // bound) and would pin any multi-turn session to ctx 100%. Prefer last,
        // falling back to total only when a rollout lacks the per-turn field.
        let usage = info
            .get("last_token_usage")
            .or_else(|| info.get("total_token_usage"));

        // For OpenAI/Codex models `input_tokens` already includes the cached
        // portion (`cached_input_tokens` is a subset detail, not a separate
        // bucket the way Anthropic reports cache_read), so it alone is the
        // window occupancy — adding cached would double-count.
        let total_input = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        if total_input == 0 {
            continue;
        }

        if let Some(window) = info.get("model_context_window").and_then(|w| w.as_u64()) {
            if window > 0 {
                return Some((total_input as f64 / window as f64).min(1.0));
            }
        }

        // Codex/GPT models — no 1M-Claude inference applies, pass 0.
        return compute_context_percent(total_input, model, 0);
    }

    None
}

/// Codex-native token breakdown for a single rollout, shown in the desktop
/// "Token" tab. Unlike Claude's `TaskTokenBreakdown` (which attributes input to
/// system-prompt / tools / CLAUDE.md / memory / MCP buckets — concepts a Codex
/// rollout has none of), this reports the raw `total_token_usage` split that
/// Codex actually records: full-price input, cached input, and output.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, PartialEq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct CodexTokenBreakdown {
    /// Full-price input tokens (the cached portion is excluded here).
    pub input_tokens: u64,
    /// Cached input tokens, billed at the model's cache-read rate. A subset of
    /// the rollout's raw `input_tokens`, so `input_tokens + cached_input_tokens`
    /// equals that raw cumulative input.
    pub cached_input_tokens: u64,
    /// Output tokens. Codex's `output_tokens` already includes reasoning tokens.
    pub output_tokens: u64,
    /// `input_tokens + cached_input_tokens + output_tokens` — the three rows sum
    /// to this so the panel's total is internally consistent.
    pub total_tokens: u64,
    /// Estimated USD cost using the model's price tier (same math as the header).
    pub cost_usd: f64,
    /// Context-window utilization (0.0..=1.0) from the latest `token_count`, when
    /// a `model_context_window` is present; `None` otherwise.
    pub context_percent: Option<f64>,
    /// Model id used for pricing, when resolvable from the rollout.
    pub model: Option<String>,
}

/// Pull the latest cumulative `total_token_usage` from a parsed rollout,
/// returning `(raw_input_incl_cached, cached, output)`. Mirrors the reverse scan
/// in [`codex_cost_and_input`] so the breakdown and the cost agree on which
/// `token_count` event is canonical.
fn latest_total_token_usage(lines: &[Value]) -> Option<(u64, u64, u64)> {
    for line in lines.iter().rev() {
        if line.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
            continue;
        }
        let payload = line.get("payload")?;
        if payload.get("type").and_then(|t| t.as_str()) != Some("token_count") {
            continue;
        }
        let usage = match payload.get("info").and_then(|i| i.get("total_token_usage")) {
            Some(u) => u,
            None => continue,
        };
        let input = usage
            .get("input_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cached = usage
            .get("cached_input_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0)
            .min(input);
        let output = usage
            .get("output_tokens")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        if input == 0 && output == 0 {
            continue;
        }
        return Some((input, cached, output));
    }
    None
}

/// Build the Codex-native [`CodexTokenBreakdown`] for a `codex://` rollout URI.
/// Returns `Ok` with zeroed token counts (cost 0, no context%) when the rollout
/// exists but has no `token_count` event yet — the panel then simply shows a
/// zero row rather than an error. Errors only on an unreadable/absent file.
pub fn codex_token_breakdown(uri: &str) -> Result<CodexTokenBreakdown, String> {
    let file_path = resolve_uri(uri).ok_or_else(|| format!("Invalid Codex URI: {uri}"))?;
    let content = read_session_content(&file_path)?;
    let lines: Vec<Value> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let model = extract_model(&lines);
    Ok(codex_token_breakdown_from_lines(&lines, model))
}

/// Pure assembly of a [`CodexTokenBreakdown`] from parsed rollout lines. Split
/// out from [`codex_token_breakdown`] so the token math is unit-testable without
/// touching the filesystem.
fn codex_token_breakdown_from_lines(lines: &[Value], model: Option<String>) -> CodexTokenBreakdown {
    let model_ref = model.as_deref();
    let (raw_input, cached, output) = latest_total_token_usage(lines).unwrap_or((0, 0, 0));
    // `raw_input` is cumulative and already includes `cached`; split it into the
    // full-price portion for display so the three rows sum to `total_tokens`.
    let full_price_input = raw_input.saturating_sub(cached);
    let (cost_usd, _) = codex_cost_and_input(lines, model_ref);
    let context_percent = extract_context_percent(lines, model_ref);

    CodexTokenBreakdown {
        input_tokens: full_price_input,
        cached_input_tokens: cached,
        output_tokens: output,
        total_tokens: raw_input + output,
        cost_usd,
        context_percent,
        model,
    }
}

/// Scan codex processes to find PIDs, including thread IDs from command-line args.
fn scan_codex_processes() -> Vec<CodexProcess> {
    use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

    let mut result = Vec::new();
    let mut sys = System::new();

    // Phase 1: scan all processes for cmd only (no cwd) to avoid triggering
    // macOS TCC permission dialogs for unrelated processes whose cwd may be
    // in protected directories (~/Documents, ~/Music, network volumes, etc.).
    sys.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always),
    );

    // Collect matched PIDs and their cmd args before phase 2.
    let matched: Vec<_> = sys
        .processes()
        .iter()
        .filter_map(|(pid, process)| {
            let name = process.name().to_string_lossy();
            let cmd_parts: Vec<String> = process
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect();
            let cmd_str = cmd_parts.join(" ");

            let is_codex = name == "codex"
                || name == "codex.exe"
                || name == "codex-rs"
                || (name.starts_with("node") && cmd_str.contains("codex"));

            if is_codex {
                Some((*pid, cmd_parts))
            } else {
                None
            }
        })
        .collect();

    // Phase 2: read cwd only for matched processes.
    let matched_pids: Vec<_> = matched.iter().map(|(pid, _)| *pid).collect();
    if !matched_pids.is_empty() {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&matched_pids),
            true,
            ProcessRefreshKind::nothing()
                .with_cwd(UpdateKind::Always),
        );
    }

    for (pid, cmd_parts) in &matched {
        if let Some(process) = sys.process(*pid) {
            let cwd = process
                .cwd()
                .and_then(|p| p.to_str())
                .unwrap_or("")
                .to_string();
            if cwd.is_empty() {
                continue;
            }

            let thread_id = extract_thread_id_from_args(cmd_parts);

            result.push(CodexProcess {
                pid: pid.as_u32(),
                cwd,
                thread_id,
            });
        }
    }
    result
}

/// Extract thread/session ID from codex command-line arguments.
/// Looks for patterns like `--thread <uuid>`, `--resume <uuid>`, `-t <uuid>`.
fn extract_thread_id_from_args(args: &[String]) -> Option<String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            // `--thread <id>` / `--resume <id>` / `-t <id>` flags, and the
            // `resume` subcommand positional — `codex exec resume <id> …`, the
            // exact shape Fleet's own resume launcher builds
            // (`build_codex_resume_args`). Without the positional case a live
            // Fleet resume wouldn't be thread-matched, so `proc_alive` stayed
            // false for it.
            "--thread" | "--resume" | "-t" | "resume" => {
                if let Some(id) = iter.next() {
                    if !id.starts_with('-') {
                        return Some(id.clone());
                    }
                }
            }
            _ => {
                // Handle --thread=<id> format.
                if let Some(rest) = arg.strip_prefix("--thread=") {
                    return Some(rest.to_string());
                }
                if let Some(rest) = arg.strip_prefix("--resume=") {
                    return Some(rest.to_string());
                }
            }
        }
    }
    None
}

/// Parsed Codex source info.
struct SourceInfo {
    ide_name: Option<String>,
    is_subagent: bool,
    parent_thread_id: Option<String>,
    agent_nickname: Option<String>,
    agent_role: Option<String>,
}

/// Parse the Codex `source` field, which can be either a plain string like "vscode"
/// or a JSON object like `{"subagent":{"thread_spawn":{"parent_thread_id":"...","agent_nickname":"Goodall","agent_role":"explorer"}}}`.
fn parse_source(source: &str) -> SourceInfo {
    // Try parsing as JSON first (subagent case).
    if let Ok(parsed) = serde_json::from_str::<Value>(source) {
        if let Some(spawn) = parsed
            .get("subagent")
            .and_then(|s| s.get("thread_spawn"))
        {
            return SourceInfo {
                ide_name: None,
                is_subagent: true,
                parent_thread_id: spawn
                    .get("parent_thread_id")
                    .and_then(|p| p.as_str())
                    .map(|s| s.to_string()),
                agent_nickname: spawn
                    .get("agent_nickname")
                    .and_then(|n| n.as_str())
                    .map(|s| s.to_string()),
                agent_role: spawn
                    .get("agent_role")
                    .and_then(|r| r.as_str())
                    .map(|s| s.to_string()),
            };
        }
    }

    // Plain string source.
    let ide_name = match source.to_lowercase().as_str() {
        "vscode" | "vs_code" => Some("VS Code".to_string()),
        "jetbrains" => Some("JetBrains".to_string()),
        "xcode" => Some("Xcode".to_string()),
        _ => None,
    };
    // NOTE: plain "exec" is NOT a subagent — it is the headless mode Fleet
    // launches every top-level codex session with (`codex exec`), so ~all
    // Fleet-owned codex sessions carry source="exec". Real codex subagents
    // record a JSON source (`{"subagent":{"thread_spawn":...}}`) and are caught
    // by the JSON branch above. Classifying "exec" as a subagent set
    // is_subagent=true, which the desktop's `adhocSessions`
    // (`!isSubagent && isFleetOwnedEntrypoint`) then filtered out — the New
    // Session launcher hung forever on "正在启动会话…" because matchSpawnedSession
    // could never find the spawned codex session.
    let is_subagent = matches!(source.to_lowercase().as_str(), "sub_agent" | "subagent");

    SourceInfo {
        ide_name,
        is_subagent,
        parent_thread_id: None,
        agent_nickname: None,
        agent_role: None,
    }
}

// ── SQLite-based scanning ────────────────────────────────────────────────────

/// Read thread metadata from Codex's SQLite database.
fn read_threads_from_sqlite() -> Option<Vec<SqliteThread>> {
    let db_path = get_sqlite_path()?;
    if !db_path.exists() {
        return None;
    }

    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .ok()?;

    // Read threads updated in the last 7 days, non-archived.
    let cutoff = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
        - 7 * 24 * 3600;

    let mut stmt = conn
        .prepare(
            "SELECT id, rollout_path, created_at, updated_at, source, cwd, title,
                    model, tokens_used, agent_nickname, agent_role, archived,
                    first_user_message
             FROM threads
             WHERE updated_at > ?1 AND archived = 0
             ORDER BY updated_at DESC",
        )
        .ok()?;

    let rows = stmt
        .query_map([cutoff], |row| {
            Ok(SqliteThread {
                id: row.get(0)?,
                rollout_path: row.get(1)?,
                created_at: row.get(2)?,
                updated_at: row.get(3)?,
                source: row.get(4)?,
                cwd: row.get(5)?,
                title: row.get(6)?,
                model: row.get(7)?,
                tokens_used: row.get(8)?,
                agent_nickname: row.get(9)?,
                agent_role: row.get(10)?,
                archived: row.get::<_, i64>(11)? != 0,
                first_user_message: row.get(12)?,
            })
        })
        .ok()?;

    Some(rows.filter_map(|r| r.ok()).collect())
}

/// Build a SessionInfo from SQLite metadata, enriching with rollout data for active sessions.
fn build_session_from_sqlite(
    thread: &SqliteThread,
    codex_processes: &[CodexProcess],
) -> Option<SessionInfo> {
    let rollout_path = PathBuf::from(&thread.rollout_path);
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let age_secs = (now_secs - thread.updated_at).max(0) as f64;

    let created_at_ms = (thread.created_at as u64) * 1000;
    let mut last_activity_ms = (thread.updated_at as u64) * 1000;

    // For recently active sessions, read the rollout file for precise status.
    let (status, token_speed, total_output_tokens, last_message_preview, model, thinking_level, context_percent, rate_limit, codex_cost, total_input_tokens) =
        if age_secs < 600.0 && rollout_path.exists() {
            // Update last_activity_ms from file mtime for sub-second precision.
            if let Ok(meta) = fs::metadata(&rollout_path) {
                if let Ok(mtime) = meta.modified() {
                    let file_age = SystemTime::now()
                        .duration_since(mtime)
                        .unwrap_or(Duration::from_secs(3600));
                    last_activity_ms = mtime
                        .duration_since(UNIX_EPOCH)
                        .ok()
                        .map(|d| d.as_millis() as u64)
                        .unwrap_or(last_activity_ms);

                    if let Ok(content) = read_session_content(&rollout_path) {
                        let all_parsed: Vec<Value> = content
                            .lines()
                            .filter_map(|l| serde_json::from_str(l).ok())
                            .collect();

                        let last_n_start = all_parsed.len().saturating_sub(100);
                        let last_n = &all_parsed[last_n_start..];

                        let st = determine_status(last_n, file_age.as_secs_f64());
                        // Rate-limit from this session's own rollout (Fleet-owned +
                        // cut-off + reached window). Overrides status so auto-resume
                        // can continue it after reset.
                        let originator_in = extract_session_meta(&all_parsed)
                            .and_then(|m| m.get("originator").and_then(|o| o.as_str()))
                            .filter(|s| !s.is_empty());
                        let rl = codex_rollout_rate_limit(
                            &all_parsed,
                            originator_in,
                            chrono::Utc::now(),
                        );
                        let st = if rl.is_some() {
                            crate::session::SessionStatus::RateLimited
                        } else {
                            st
                        };
                        let (spd, tok) = compute_token_stats(&all_parsed);
                        let preview = extract_last_text(last_n);
                        let mdl = extract_model(&all_parsed).or_else(|| thread.model.clone());

                        let has_reasoning = last_n.iter().any(|v| {
                            v.get("type").and_then(|t| t.as_str()) == Some("response_item")
                                && v.get("payload")
                                    .and_then(|p| p.get("type"))
                                    .and_then(|t| t.as_str())
                                    == Some("reasoning")
                        });
                        let tl = if has_reasoning {
                            Some("thinking".to_string())
                        } else {
                            None
                        };
                        let tok = if tok > 0 { tok } else { thread.tokens_used as u64 };
                        let ctx = extract_context_percent(&all_parsed, mdl.as_deref());
                        let (cost, input) = codex_cost_and_input(&all_parsed, mdl.as_deref());
                        (st, spd, tok, preview, mdl, tl, ctx, rl, cost, input)
                    } else {
                        (
                            determine_status_from_age(file_age.as_secs_f64()),
                            0.0,
                            thread.tokens_used as u64,
                            None,
                            thread.model.clone(),
                            None,
                            None,
                            None,
                            0.0,
                            0,
                        )
                    }
                } else {
                    (
                        determine_status_from_age(age_secs),
                        0.0,
                        thread.tokens_used as u64,
                        None,
                        thread.model.clone(),
                        None,
                        None,
                        None,
                        0.0,
                        0,
                    )
                }
            } else {
                (
                    determine_status_from_age(age_secs),
                    0.0,
                    thread.tokens_used as u64,
                    None,
                    thread.model.clone(),
                    None,
                    None,
                    None,
                    0.0,
                    0,
                )
            }
        } else {
            // Older session — use SQLite metadata only, skip rollout file.
            let preview = if !thread.first_user_message.is_empty() {
                Some(thread.first_user_message.chars().take(200).collect())
            } else if !thread.title.is_empty() {
                Some(thread.title.clone())
            } else {
                None
            };
            (
                determine_status_from_age(age_secs),
                0.0,
                thread.tokens_used as u64,
                preview,
                thread.model.clone(),
                None,
                None,
                None,
                0.0,
                0,
            )
        };

    let workspace_name = PathBuf::from(&thread.cwd)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("codex")
        .to_string();

    // Parse the source field (may be plain string or JSON for subagents).
    let source_info = parse_source(&thread.source);

    // PID resolution: prefer thread-id match, fall back to workspace path match.
    let (pid, pid_precise) = resolve_pid(codex_processes, &thread.id, &thread.cwd);
    let proc_alive = codex_proc_alive(codex_processes, &thread.id);

    // Codex stores only the raw first prompt as `title`; for non-subagent
    // threads Fleet generates a semantic title from the first user message
    // (background, cached). Kick that off and prefer any cached result.
    if !source_info.is_subagent {
        crate::codex_title::maybe_generate(&thread.id, &thread.first_user_message);
    }

    // Prefer: source-embedded nickname > SQLite agent_nickname
    //       > Fleet-generated semantic title > raw codex title (first prompt)
    let ai_title = source_info
        .agent_nickname
        .or_else(|| thread.agent_nickname.clone())
        .or_else(|| crate::codex_title::cached_title(&thread.id))
        .or_else(|| {
            if !thread.title.is_empty() {
                Some(thread.title.clone())
            } else {
                None
            }
        });

    let agent_type = source_info
        .agent_role
        .or_else(|| thread.agent_role.clone());

    // Fleet-ownership marker: the rollout `originator` (see
    // `read_rollout_originator`). Fleet-launched Codex sessions carry
    // `originator == "fleet"`, which `is_fleet_owned_entrypoint` recognises.
    let entrypoint = read_rollout_originator(&rollout_path);

    let uri = build_uri(&rollout_path)?;

    Some(SessionInfo {
        id: thread.id.clone(),
        workspace_path: thread.cwd.clone(),
        workspace_name,
        ide_name: source_info.ide_name,
        entrypoint,
        is_subagent: source_info.is_subagent,
        parent_session_id: source_info.parent_thread_id,
        agent_type,
        agent_description: None,
        slug: None,
        ai_title,
        status,
        token_speed,
        agent_token_speed: token_speed,
        total_output_tokens,
        total_input_tokens,
        total_cost_usd: codex_cost,
        agent_total_cost_usd: codex_cost,
        cost_speed_usd_per_min: 0.0,
        last_message_preview,
        last_activity_ms,
        agent_last_activity_ms: last_activity_ms,
        running_subagent_count: 0,
        created_at_ms,
        jsonl_path: uri,
        model,
        thinking_level,
        pid,
        pid_precise,
        // Definitive liveness for Codex: a live process resuming exactly this
        // thread (see `codex_proc_alive`). `apply_pid_liveness` runs only in the
        // Claude scan path, so Codex stamps its own here.
        proc_alive,
        pending_tool_batch: false,
        last_skill: None,
        context_percent,
        agent_source: "codex".to_string(),
        last_outcome: None,
        rate_limit,
        todos: None,
        background_tasks: Vec::new(),
        task_plan: None, handoff: None, user_mark: None, last_read_ms: None,        compact_count: 0,
        compact_pre_tokens: 0,
        compact_post_tokens: 0,
        compact_cost_usd: 0.0,
        pending_messages: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::{
        codex_cost_and_input, codex_last_turn_incomplete, codex_rate_limit_state_from_rollout,
        codex_rate_limit_state_from_usage, codex_rollout_rate_limit,
        codex_token_breakdown_from_lines, compute_token_stats, extract_context_percent,
        extract_first_user_text, last_rollout_rate_limits, latest_total_token_usage,
        normalize_messages, read_rollout_originator, CodexRateLimitWindow, CodexUsageItem,
    };
    use serde_json::json;

    #[test]
    fn normalize_messages_dedups_agent_message_against_response_item() {
        // A single Codex turn persists the SAME assistant text twice:
        //   - event_msg/agent_message  (UI event-timeline mirror)
        //   - response_item/message    (canonical model-history item — what
        //     Codex's own resume path replays)
        // The transcript must render the reply exactly once.
        let lines = vec![
            json!({"type":"event_msg","payload":{"type":"user_message","message":"hi"},"timestamp":"t0"}),
            json!({"type":"response_item","payload":{"type":"reasoning","summary":[{"text":"thinking..."}]},"timestamp":"t1"}),
            json!({"type":"event_msg","payload":{"type":"agent_message","message":"hello world"},"timestamp":"t2"}),
            json!({"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"hello world"}]},"timestamp":"t3"}),
        ];

        let out = normalize_messages(lines);

        let assistant_texts: Vec<String> = out
            .iter()
            .filter(|m| m.get("type").and_then(|t| t.as_str()) == Some("assistant"))
            .filter_map(|m| {
                let content = m.get("message")?.get("content")?.as_array()?;
                content.iter().find_map(|b| {
                    if b.get("type").and_then(|t| t.as_str()) == Some("text") {
                        b.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                    } else {
                        None
                    }
                })
            })
            .collect();

        assert_eq!(
            assistant_texts,
            vec!["hello world".to_string()],
            "assistant reply should render exactly once, got: {assistant_texts:?}"
        );
    }

    #[test]
    fn normalize_messages_keeps_agent_message_without_response_item() {
        // Interrupted / not-yet-persisted turn: only the agent_message event
        // exists, no matching response_item. The reply must NOT be dropped.
        let lines = vec![
            json!({"type":"event_msg","payload":{"type":"agent_message","message":"partial reply"},"timestamp":"t0"}),
        ];

        let out = normalize_messages(lines);

        let assistant_texts: Vec<String> = out
            .iter()
            .filter(|m| m.get("type").and_then(|t| t.as_str()) == Some("assistant"))
            .filter_map(|m| {
                let content = m.get("message")?.get("content")?.as_array()?;
                content.iter().find_map(|b| {
                    b.get("text").and_then(|t| t.as_str()).map(|s| s.to_string())
                })
            })
            .collect();

        assert_eq!(assistant_texts, vec!["partial reply".to_string()]);
    }

    #[test]
    fn normalize_messages_dedups_user_message_against_response_item() {
        // Current Codex rollouts persist a real prompt in both forms. The
        // response_item is canonical and the event_msg is only its timeline
        // mirror, so Fleet must render one user bubble rather than two.
        let lines = vec![
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "same prompt"}]
                },
                "timestamp": "t0"
            }),
            json!({
                "type": "event_msg",
                "payload": {"type": "user_message", "message": "same prompt"},
                "timestamp": "t1"
            }),
        ];

        let out = normalize_messages(lines);
        let user_texts: Vec<&str> = out
            .iter()
            .filter(|m| m.get("type").and_then(|t| t.as_str()) == Some("user"))
            .filter_map(|m| {
                m.get("message")?
                    .get("content")?
                    .as_array()?
                    .first()?
                    .get("text")?
                    .as_str()
            })
            .collect();

        assert_eq!(user_texts, vec!["same prompt"]);
    }

    #[test]
    fn normalize_messages_keeps_unmatched_repeated_user_event() {
        // A count (not a global text set) ensures an event-only partial turn is
        // retained even when it repeats a prompt from a completed older turn.
        let lines = vec![
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{"type": "input_text", "text": "again"}]
                }
            }),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"again"}}),
            json!({"type":"event_msg","payload":{"type":"user_message","message":"again"}}),
        ];

        let out = normalize_messages(lines);
        assert_eq!(
            out.iter()
                .filter(|m| m.get("type").and_then(|t| t.as_str()) == Some("user"))
                .count(),
            2
        );
    }

    #[test]
    fn extract_first_user_text_finds_earliest_user_turn() {
        // event_msg user_message form, preceded by a non-user line.
        let lines = vec![
            json!({"type": "event_msg", "payload": {"type": "session_meta"}}),
            json!({"type": "event_msg", "payload": {"type": "user_message", "message": "first ask"}}),
            json!({"type": "event_msg", "payload": {"type": "user_message", "message": "second ask"}}),
        ];
        assert_eq!(extract_first_user_text(&lines).as_deref(), Some("first ask"));

        // response_item message/user with input_text content block.
        let lines2 = vec![json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{"type": "input_text", "text": "help me refactor"}]
            }
        })];
        assert_eq!(
            extract_first_user_text(&lines2).as_deref(),
            Some("help me refactor")
        );

        // Assistant turns and empty messages are ignored.
        let lines3 = vec![
            json!({"type": "event_msg", "payload": {"type": "agent_message", "message": "hi"}}),
            json!({"type": "event_msg", "payload": {"type": "user_message", "message": "   "}}),
            json!({"type": "event_msg", "payload": {"type": "user_message", "message": "real one"}}),
        ];
        assert_eq!(extract_first_user_text(&lines3).as_deref(), Some("real one"));

        assert_eq!(extract_first_user_text(&[]), None);
    }

    #[test]
    fn developer_role_message_is_tagged_is_meta() {
        // Codex injects its sandbox/permissions preamble, the `/root`
        // multi-agent collaboration prompt, and `<multi_agent_mode>` guidance
        // as `role:"developer"` response_items. These are codex-native
        // boilerplate, not user turns, so normalize_messages must flag them
        // `isMeta` (mirroring Claude's harness-injected records) — the frontend
        // folds `isMeta` messages into a collapsed card instead of a bubble.
        let lines = vec![
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "developer",
                    "content": [{
                        "type": "input_text",
                        "text": "You are `/root`, the primary agent in a team of agents."
                    }]
                }
            }),
            json!({
                "type": "response_item",
                "payload": {
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": "real user prompt" }]
                }
            }),
        ];
        let out = normalize_messages(lines);
        assert_eq!(out.len(), 2, "both messages emitted");

        // Developer message: type user (right-aligned lane) but flagged isMeta.
        assert_eq!(out[0]["type"], json!("user"));
        assert_eq!(out[0]["message"]["role"], json!("developer"));
        assert_eq!(
            out[0]["isMeta"],
            json!(true),
            "developer-role message must be tagged isMeta so the frontend folds it"
        );

        // A genuine user turn stays un-flagged.
        assert_eq!(out[1]["type"], json!("user"));
        assert!(
            out[1].get("isMeta").is_none(),
            "real user messages must NOT be tagged isMeta"
        );
    }

    #[test]
    fn recommended_plugins_user_context_is_tagged_is_meta() {
        // Codex injects this runtime block with role=user (unlike its other
        // role=developer boilerplate), but it still must render as a collapsed
        // system-context row rather than as a user-authored chat bubble.
        let lines = vec![json!({
            "type": "response_item",
            "payload": {
                "type": "message",
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": "<recommended_plugins>\n- GitHub\n</recommended_plugins><environment_context>...</environment_context>"
                }]
            }
        })];

        let out = normalize_messages(lines);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0]["isMeta"], json!(true));
    }

    #[test]
    fn parse_source_exec_is_top_level_not_subagent() {
        // Fleet launches every codex session via `codex exec`, so the SQLite
        // `threads.source` column is the plain string "exec". That is a
        // TOP-LEVEL session, not a subagent — real codex subagents record a
        // JSON source (`{"subagent":{"thread_spawn":...}}`), handled by the JSON
        // branch below. Misclassifying plain "exec" as a subagent set
        // is_subagent=true, which the desktop's `adhocSessions` filter
        // (`!isSubagent && isFleetOwnedEntrypoint`) then excluded — so a
        // Fleet-spawned codex "新会话" never surfaced and the launcher hung
        // forever on "正在启动会话…".
        assert!(!super::parse_source("exec").is_subagent, "plain exec is top-level");
        assert!(!super::parse_source("EXEC").is_subagent, "case-insensitive");
        // Interactive TUI ("cli") and IDE sources are top-level too.
        assert!(!super::parse_source("cli").is_subagent);
        assert!(!super::parse_source("vscode").is_subagent);
        // Real subagents (JSON source) MUST still classify as subagents.
        let sub = super::parse_source(
            r#"{"subagent":{"thread_spawn":{"parent_thread_id":"019d","agent_nickname":"Goodall","agent_role":"explorer"}}}"#,
        );
        assert!(sub.is_subagent, "JSON subagent source stays a subagent");
        assert_eq!(sub.parent_thread_id.as_deref(), Some("019d"));
    }

    #[test]
    fn codex_cost_and_input_prices_last_token_count() {
        // Real gpt-5.6-sol usage shape: cached is a subset of input, output
        // already includes reasoning. An earlier token_count is present to
        // prove we read the LAST (cumulative) one, not the first.
        let lines = vec![
            json!({
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": { "total_token_usage": {
                        "input_tokens": 1000, "cached_input_tokens": 0, "output_tokens": 10
                    }}
                }
            }),
            json!({
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": { "total_token_usage": {
                        "input_tokens": 136970,
                        "cached_input_tokens": 123392,
                        "output_tokens": 565,
                        "reasoning_output_tokens": 60,
                        "total_tokens": 137535
                    }}
                }
            }),
        ];
        let (cost, input) = codex_cost_and_input(&lines, Some("gpt-5.6-sol"));
        assert_eq!(input, 136970, "reports cumulative input");
        // (136970-123392)/1M*$5 + 123392/1M*$0.50 + 565/1M*$30
        let expected = 13578.0 / 1e6 * 5.0 + 123392.0 / 1e6 * 0.50 + 565.0 / 1e6 * 30.0;
        assert!(
            (cost - expected).abs() < 1e-9,
            "cost {cost} != expected {expected}"
        );
    }

    #[test]
    fn codex_cost_and_input_absent_model_uses_gpt_sol() {
        // No model -> "gpt" -> Sol tier. 1M full-price input = $5.
        let lines = vec![json!({
            "type": "event_msg",
            "payload": { "type": "token_count", "info": { "total_token_usage": {
                "input_tokens": 1_000_000, "cached_input_tokens": 0, "output_tokens": 0
            }}}
        })];
        let (cost, input) = codex_cost_and_input(&lines, None);
        assert_eq!(input, 1_000_000);
        assert!((cost - 5.0).abs() < 1e-9, "gpt sol input = $5/M, got {cost}");
    }

    #[test]
    fn codex_cost_and_input_no_token_count_is_zero() {
        let lines = vec![json!({ "type": "response_item", "payload": {} })];
        assert_eq!(codex_cost_and_input(&lines, Some("gpt-5.6-sol")), (0.0, 0));
    }

    #[test]
    fn latest_total_token_usage_reads_last_cumulative_and_clamps_cached() {
        // Two token_count events prove we read the LAST (cumulative) one, and
        // cached is clamped to input so `input - cached` can never underflow.
        let lines = vec![
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":10}}}}),
            json!({"type":"event_msg","payload":{"type":"token_count","info":{
                "total_token_usage":{"input_tokens":136970,"cached_input_tokens":999999,"output_tokens":565}}}}),
        ];
        // cached (999999) is clamped down to input (136970).
        assert_eq!(latest_total_token_usage(&lines), Some((136970, 136970, 565)));
    }

    #[test]
    fn latest_total_token_usage_none_without_token_count() {
        let lines = vec![json!({"type":"response_item","payload":{}})];
        assert_eq!(latest_total_token_usage(&lines), None);
    }

    #[test]
    fn codex_token_breakdown_splits_full_price_and_sums_to_total() {
        // raw input (incl. cached) = 136970, cached = 123392, output = 565.
        // Panel must show full-price input = 136970 - 123392 = 13578, and the
        // three rows (13578 + 123392 + 565) must sum to total_tokens 137535.
        let lines = vec![json!({"type":"event_msg","payload":{"type":"token_count","info":{
            "total_token_usage":{
                "input_tokens":136970,"cached_input_tokens":123392,"output_tokens":565
            }}}})];
        let b = codex_token_breakdown_from_lines(&lines, Some("gpt-5.6-sol".to_string()));
        assert_eq!(b.input_tokens, 13578);
        assert_eq!(b.cached_input_tokens, 123392);
        assert_eq!(b.output_tokens, 565);
        assert_eq!(b.total_tokens, 137535);
        assert_eq!(
            b.input_tokens + b.cached_input_tokens + b.output_tokens,
            b.total_tokens
        );
        // Same cost math as the header's codex_cost_and_input.
        let expected = 13578.0 / 1e6 * 5.0 + 123392.0 / 1e6 * 0.50 + 565.0 / 1e6 * 30.0;
        assert!((b.cost_usd - expected).abs() < 1e-9, "cost {}", b.cost_usd);
    }

    #[test]
    fn codex_token_breakdown_zeroed_when_no_token_count() {
        // A rollout that exists but hasn't emitted a token_count yet yields an
        // all-zero breakdown (the panel shows a zero row, not an error).
        let lines = vec![json!({"type":"response_item","payload":{}})];
        let b = codex_token_breakdown_from_lines(&lines, None);
        assert_eq!(b.total_tokens, 0);
        assert_eq!(b.cost_usd, 0.0);
        assert_eq!(b.context_percent, None);
    }

    /// The scanner's Fleet-ownership signal: the rollout's first `session_meta`
    /// line carries `originator`, and Fleet stamps `"fleet"` there via
    /// `CODEX_INTERNAL_ORIGINATOR_OVERRIDE`. `read_rollout_originator` must pull
    /// exactly that value (and only from a real `session_meta` line).
    #[test]
    fn read_rollout_originator_pulls_session_meta_originator() {
        use std::io::Write;
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            f,
            r#"{{"type":"session_meta","payload":{{"id":"t1","originator":"fleet","source":"exec"}}}}"#
        )
        .unwrap();
        writeln!(f, r#"{{"type":"turn_context","payload":{{"model":"gpt-5"}}}}"#).unwrap();
        assert_eq!(
            read_rollout_originator(f.path()).as_deref(),
            Some("fleet"),
            "must read originator from the first session_meta line"
        );
    }

    #[test]
    fn extract_thread_id_handles_exec_resume_positional() {
        use super::extract_thread_id_from_args;
        // Fleet's own resume argv (build_codex_resume_args): the thread id is a
        // positional after the `resume` subcommand, not a `--resume` flag. This
        // is the shape a live Fleet auto-resume / handoff-resume process has, so
        // proc_alive depends on parsing it.
        let argv: Vec<String> = ["exec", "resume", "019f-thread-xyz", "--json", "--skip-git-repo-check", "--", "hi"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            extract_thread_id_from_args(&argv).as_deref(),
            Some("019f-thread-xyz")
        );
        // The flag forms must keep working.
        let flagged: Vec<String> = ["--resume", "abc"].iter().map(|s| s.to_string()).collect();
        assert_eq!(extract_thread_id_from_args(&flagged).as_deref(), Some("abc"));
    }

    #[test]
    fn codex_proc_alive_only_on_exact_thread_match() {
        use super::{codex_proc_alive, CodexProcess};
        let procs = vec![
            CodexProcess { pid: 10, cwd: "/ws".into(), thread_id: Some("t-alive".into()) },
            // Same cwd, different thread — must NOT make t-dead look alive (the
            // pid-per-session hazard a cwd match would fall into).
            CodexProcess { pid: 11, cwd: "/ws".into(), thread_id: None },
        ];
        assert!(codex_proc_alive(&procs, "t-alive"));
        assert!(!codex_proc_alive(&procs, "t-dead"));
        assert!(!codex_proc_alive(&[], "t-alive"));
    }

    /// A freshly spawned `codex exec` mints its thread id *after* launch, so that
    /// id is in no argv while its first turn runs — the argv match above reads it
    /// as dead. The spawn-pid note closes that gap: liveness falls back to "the
    /// pid Fleet recorded at spawn is still a live Codex process". Without this,
    /// the enqueue-drain gate fires a second `resume` on a still-running new
    /// session and corrupts its transcript (M5 P15).
    #[test]
    fn codex_proc_alive_recognises_running_spawn_via_pid_note() {
        use super::{codex_proc_alive, CodexProcess};
        let _lock = crate::session::fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-spawnpid-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        std::env::set_var("FLEET_HOME", &tmp);

        crate::codex_launch::record_spawn_pid("t-new", 4242);

        // The live Codex set has that pid but no thread id in argv — exactly what
        // a mid-first-turn `codex exec` looks like.
        let running = vec![CodexProcess { pid: 4242, cwd: "/ws".into(), thread_id: None }];
        assert!(
            codex_proc_alive(&running, "t-new"),
            "a running new spawn must read as alive via its recorded pid"
        );
        // The pid leaving the live Codex set (session exited) makes it drainable.
        assert!(
            !codex_proc_alive(&[], "t-new"),
            "spawn pid no longer live => not alive"
        );
        // A thread with no note must not borrow another session's liveness.
        assert!(!codex_proc_alive(&running, "t-unknown"));

        match prev {
            Some(p) => std::env::set_var("FLEET_HOME", p),
            None => std::env::remove_var("FLEET_HOME"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn read_rollout_originator_none_without_meta_or_field() {
        use std::io::Write;
        // First line is not session_meta → None (we only trust line 1).
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, r#"{{"type":"turn_context","payload":{{"model":"gpt-5"}}}}"#).unwrap();
        assert_eq!(read_rollout_originator(f.path()), None);

        // session_meta without an originator field → None.
        let mut g = tempfile::NamedTempFile::new().unwrap();
        writeln!(g, r#"{{"type":"session_meta","payload":{{"id":"t2"}}}}"#).unwrap();
        assert_eq!(read_rollout_originator(g.path()), None);
    }

    fn window(used_percent: i32, window_mins: i64, resets_at_secs: i64) -> CodexRateLimitWindow {
        CodexRateLimitWindow {
            used_percent,
            window_duration_mins: Some(window_mins),
            resets_at: Some(resets_at_secs),
        }
    }

    #[test]
    fn codex_rate_limit_state_none_when_not_reached() {
        // No `rateLimitReachedType` → account not limited → no state.
        let usage = CodexUsageItem {
            primary: Some(window(2, 10080, 1_784_636_186)),
            rate_limit_reached_type: None,
            ..Default::default()
        };
        assert!(codex_rate_limit_state_from_usage(&usage, chrono::Utc::now()).is_none());
    }

    #[test]
    fn codex_rate_limit_state_converts_resets_at_seconds() {
        // resetsAt is epoch SECONDS (measured against codex-cli 0.144.4), not ms.
        let resets_secs = 1_784_636_186_i64;
        let usage = CodexUsageItem {
            primary: Some(window(100, 10080, resets_secs)),
            rate_limit_reached_type: Some("primary".into()),
            ..Default::default()
        };
        let st = codex_rate_limit_state_from_usage(&usage, chrono::Utc::now())
            .expect("reached → Some");
        assert_eq!(
            st.resets_at.timestamp(),
            resets_secs,
            "resets_at must be interpreted as epoch seconds"
        );
        // error_timestamp = window start = resets_at - 7d (10080 min).
        assert_eq!(
            (st.resets_at - st.error_timestamp),
            chrono::Duration::minutes(10080)
        );
        // Unknown limit_type → skips the claude-metric recovery gate.
        assert_eq!(st.limit_type, crate::rate_limit_parser::RateLimitType::Unknown);
    }

    #[test]
    fn codex_rate_limit_state_prefers_secondary_when_reached() {
        let usage = CodexUsageItem {
            primary: Some(window(50, 10080, 2_000_000_000)),
            secondary: Some(window(100, 300, 1_900_000_000)),
            rate_limit_reached_type: Some("secondary".into()),
            ..Default::default()
        };
        let st = codex_rate_limit_state_from_usage(&usage, chrono::Utc::now()).unwrap();
        assert_eq!(st.resets_at.timestamp(), 1_900_000_000);
        // secondary window is 300 min = 5h.
        assert_eq!(
            (st.resets_at - st.error_timestamp),
            chrono::Duration::minutes(300)
        );
    }

    // ── rollout token_count.rate_limits (per-session) parsing ────────────────
    //
    // Shapes below are the REAL rollout `rate_limits` object (snake_case,
    // `window_minutes`, `used_percent` as float) verified against on-disk
    // codex-cli rollouts — distinct from the app-server camelCase snapshot.

    #[test]
    fn rollout_rate_limit_none_when_reached_type_null() {
        // Healthy turn: rate_limit_reached_type is present but null.
        let rl = json!({
            "limit_id": "codex", "plan_type": "team",
            "primary": {"used_percent": 3.0, "window_minutes": 10080, "resets_at": 1_784_636_185_i64},
            "secondary": null,
            "rate_limit_reached_type": null
        });
        assert!(codex_rate_limit_state_from_rollout(&rl, chrono::Utc::now()).is_none());
    }

    #[test]
    fn rollout_rate_limit_none_when_field_absent_legacy() {
        // Pre-0.14x rollout: no rate_limit_reached_type field at all.
        let rl = json!({
            "limit_id": "codex",
            "primary": {"used_percent": 26.0, "window_minutes": 300, "resets_at": 1_774_617_675_i64},
            "secondary": {"used_percent": 8.0, "window_minutes": 10080, "resets_at": 1_775_204_475_i64}
        });
        assert!(codex_rate_limit_state_from_rollout(&rl, chrono::Utc::now()).is_none());
    }

    #[test]
    fn rollout_rate_limit_secondary_reached_builds_state() {
        // A genuinely rate-limited turn: secondary (5h) window reached.
        let rl = json!({
            "limit_id": "codex", "plan_type": "team",
            "primary": {"used_percent": 50.0, "window_minutes": 10080, "resets_at": 2_000_000_000_i64},
            "secondary": {"used_percent": 100.0, "window_minutes": 300, "resets_at": 1_900_000_000_i64},
            "rate_limit_reached_type": "secondary"
        });
        let st = codex_rate_limit_state_from_rollout(&rl, chrono::Utc::now())
            .expect("reached secondary → Some");
        // resets_at from the SECONDARY window (the reached one), epoch seconds.
        assert_eq!(st.resets_at.timestamp(), 1_900_000_000);
        // window length 300 min drives error_timestamp = resets_at - 300min.
        assert_eq!(
            (st.resets_at - st.error_timestamp),
            chrono::Duration::minutes(300)
        );
        assert_eq!(st.limit_type, crate::rate_limit_parser::RateLimitType::Unknown);
    }

    // ── scan-integration gating: codex_rollout_rate_limit ───────────────────
    //
    // Synthetic rollout event streams (the same event_msg shape the scanner
    // parses) exercise the three gates together: Fleet-owned + cut-off + reached.

    fn tc_reached(reached: Option<&str>) -> serde_json::Value {
        // A token_count event whose rate_limits reports (or not) a reached window.
        let rl = if let Some(r) = reached {
            json!({
                "secondary": {"used_percent": 100.0, "window_minutes": 300, "resets_at": 1_900_000_000_i64},
                "rate_limit_reached_type": r
            })
        } else {
            json!({
                "secondary": {"used_percent": 20.0, "window_minutes": 300, "resets_at": 1_900_000_000_i64},
                "rate_limit_reached_type": null
            })
        };
        json!({"type":"event_msg","payload":{"type":"token_count","rate_limits": rl}})
    }
    fn ev(pt: &str) -> serde_json::Value {
        json!({"type":"event_msg","payload":{"type": pt}})
    }

    #[test]
    fn last_turn_incomplete_detects_cutoff_vs_clean() {
        // turn_started with no completion → cut off.
        assert!(codex_last_turn_incomplete(&[ev("turn_started"), tc_reached(Some("secondary"))]));
        // ...but a clean turn_complete after → not cut off.
        assert!(!codex_last_turn_incomplete(&[ev("turn_started"), ev("turn_complete")]));
        // turn_aborted → cut off.
        assert!(codex_last_turn_incomplete(&[ev("turn_aborted")]));
    }

    #[test]
    fn rollout_rate_limit_fires_for_fleet_owned_cutoff_reached() {
        // Fleet-owned + last turn cut off + secondary window reached → Some.
        let parsed = vec![ev("turn_started"), tc_reached(Some("secondary"))];
        let st = codex_rollout_rate_limit(&parsed, Some("fleet"), chrono::Utc::now())
            .expect("fleet-owned cut-off reached → Some");
        assert_eq!(st.resets_at.timestamp(), 1_900_000_000);
    }

    #[test]
    fn rollout_rate_limit_skips_non_fleet_session() {
        // Same signals but a non-Fleet session (interactive user) → never
        // auto-resumed, so no RateLimited state.
        let parsed = vec![ev("turn_started"), tc_reached(Some("secondary"))];
        assert!(codex_rollout_rate_limit(&parsed, Some("vscode"), chrono::Utc::now()).is_none());
        assert!(codex_rollout_rate_limit(&parsed, None, chrono::Utc::now()).is_none());
    }

    #[test]
    fn rollout_rate_limit_skips_cleanly_completed_session() {
        // Fleet-owned + reached, but the turn COMPLETED cleanly afterwards → the
        // session finished, not cut off → not a resume candidate.
        let parsed = vec![tc_reached(Some("secondary")), ev("turn_complete")];
        assert!(codex_rollout_rate_limit(&parsed, Some("fleet"), chrono::Utc::now()).is_none());
    }

    #[test]
    fn rollout_rate_limit_skips_when_not_reached() {
        // Fleet-owned + cut off, but no window reached → nothing to resume for.
        let parsed = vec![ev("turn_started"), tc_reached(None)];
        assert!(codex_rollout_rate_limit(&parsed, Some("fleet"), chrono::Utc::now()).is_none());
        // last_rollout_rate_limits still finds the (non-reached) snapshot.
        assert!(last_rollout_rate_limits(&parsed).is_some());
    }

    #[test]
    fn rollout_rate_limit_primary_reached_uses_window_minutes() {
        // Weekly (primary, 10080-min) reached — proves the rollout's
        // `window_minutes` key (not the app-server `windowDurationMins`) is read.
        let rl = json!({
            "primary": {"used_percent": 100.0, "window_minutes": 10080, "resets_at": 1_784_636_186_i64},
            "rate_limit_reached_type": "primary"
        });
        let st = codex_rate_limit_state_from_rollout(&rl, chrono::Utc::now()).unwrap();
        assert_eq!(st.resets_at.timestamp(), 1_784_636_186);
        assert_eq!(
            (st.resets_at - st.error_timestamp),
            chrono::Duration::minutes(10080),
            "window_minutes must map to the window duration"
        );
    }

    #[test]
    fn compute_token_stats_supports_new_token_count_shape() {
        let lines = vec![
            json!({
                "timestamp": "2026-03-27T08:23:14.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": { "output_tokens": 779 },
                        "last_token_usage": { "output_tokens": 428 }
                    }
                }
            }),
            json!({
                "timestamp": "2026-03-27T08:23:16.000Z",
                "type": "event_msg",
                "payload": {
                    "type": "token_count",
                    "info": {
                        "total_token_usage": { "output_tokens": 815 },
                        "last_token_usage": { "output_tokens": 36 }
                    }
                }
            }),
        ];

        let (_, total_output) = compute_token_stats(&lines);
        assert_eq!(total_output, 815);
    }

    #[test]
    fn compute_token_stats_supports_legacy_token_count_shape() {
        let lines = vec![json!({
            "timestamp": "2026-03-27T08:23:14.432Z",
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "output_tokens": 123
                }
            }
        })];

        let (_, total_output) = compute_token_stats(&lines);
        assert_eq!(total_output, 123);
    }

    #[test]
    fn extract_context_percent_prefers_precise_window_from_token_count() {
        // No `last_token_usage` here, so it falls back to `total_token_usage`.
        // `input_tokens` already includes the 1000 cached tokens, so occupancy
        // is 1500/3000, NOT (1500+1000)/3000 — cached must not be added.
        let lines = vec![json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 1500,
                        "cached_input_tokens": 1000
                    },
                    "model_context_window": 3000
                }
            }
        })];

        let pct = extract_context_percent(&lines, Some("gpt-5.4"));
        assert_eq!(pct, Some(0.5));
    }

    #[test]
    fn context_percent_uses_last_turn_not_cumulative_total() {
        // Real on-disk shape from a long gpt-5.6-sol session. Codex's
        // `total_token_usage` is CUMULATIVE across every turn (each turn resends
        // the whole prompt, so its input sums without bound — here 1.5M against
        // a 258K window), while `last_token_usage` is the current window
        // occupancy (92K). Using the cumulative total pinned the card to
        // ctx 100%. Also: for OpenAI/Codex models `input_tokens` already
        // includes the cached subset, so summing them double-counts.
        let lines = vec![json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "model_context_window": 258_400,
                    "total_token_usage": {
                        "input_tokens": 1_549_616,
                        "cached_input_tokens": 1_454_080,
                        "output_tokens": 6192
                    },
                    "last_token_usage": {
                        "input_tokens": 92_304,
                        "cached_input_tokens": 90_880,
                        "output_tokens": 393
                    }
                }
            }
        })];

        let pct = extract_context_percent(&lines, Some("gpt-5.6-sol")).unwrap();
        // 92_304 / 258_400 ≈ 0.357 — must not be clamped to 1.0, and must not
        // add cached on top (that would give 183_184 / 258_400 ≈ 0.709).
        assert!(
            (pct - 92_304.0 / 258_400.0).abs() < 1e-9,
            "expected last-turn occupancy, got {pct}"
        );
    }
}

/// Simple status from file age when we can't read the rollout file.
fn determine_status_from_age(age_secs: f64) -> SessionStatus {
    if age_secs < 8.0 {
        SessionStatus::Active
    } else if age_secs < 300.0 {
        SessionStatus::WaitingInput
    } else {
        SessionStatus::Idle
    }
}

/// Resolve PID for a session: prefer exact thread-id match, fall back to cwd match.
/// Whether a live Codex process is definitively running this thread — an exact
/// thread-id match on a running `codex exec resume <thread-id>` argv. This is
/// Codex's analogue of Claude's `exact_proc_alive` ("a live process whose argv
/// names exactly this session id"): unambiguous, so it never mis-attributes a
/// sibling session's process the way a cwd match can (see the pid-per-session
/// caveat in `resolve_pid`).
///
/// Only *resumed* sessions carry their thread id in argv. A freshly spawned
/// `codex exec` does not — Codex mints the id after launch — so a new session
/// mid-first-turn is invisible to the argv match. For those, Fleet's spawn-pid
/// note (`codex_launch::record_spawn_pid`) closes the gap: liveness falls back
/// to "the pid recorded at spawn is still a live Codex process". Matching that
/// pid against `processes` (the live Codex set) makes it immune to pid reuse — a
/// recycled pid owned by some other process simply is not in the set. Without
/// this, the enqueue-drain gate (`pending_message`, M5 P15) reads a still-running
/// new session as idle and fires a second `resume` on it, corrupting the
/// transcript.
fn codex_proc_alive(processes: &[CodexProcess], thread_id: &str) -> bool {
    // Resumed sessions: exact thread-id argv match.
    if processes
        .iter()
        .any(|p| p.thread_id.as_deref() == Some(thread_id))
    {
        return true;
    }
    // Freshly-spawned sessions: the recorded spawn pid still in the live set.
    if let Some(pid) = crate::codex_launch::resolve_spawn_pid(thread_id) {
        return processes.iter().any(|p| p.pid == pid);
    }
    false
}

/// A fresh liveness check for one Codex thread: scans the live Codex process set
/// and applies [`codex_proc_alive`]. Used as the drain gate's belt-and-braces —
/// the `SessionInfo.proc_alive` snapshot the gate is handed can lag a turn that
/// started since the scan, so this re-checks against the current process table.
pub fn codex_session_alive(thread_id: &str) -> bool {
    codex_proc_alive(&scan_codex_processes(), thread_id)
}

fn resolve_pid(processes: &[CodexProcess], thread_id: &str, cwd: &str) -> (Option<u32>, bool) {
    // First: exact thread-id match (most precise).
    for p in processes {
        if let Some(ref tid) = p.thread_id {
            if tid == thread_id {
                return (Some(p.pid), true);
            }
        }
    }
    // Second: cwd match. Precise only if exactly one process matches.
    let cwd_matches: Vec<_> = processes.iter().filter(|p| p.cwd == cwd).collect();
    match cwd_matches.len() {
        1 => (Some(cwd_matches[0].pid), true),
        n if n > 1 => (Some(cwd_matches[0].pid), false),
        _ => (None, false),
    }
}

/// Parse a single rollout file into a SessionInfo (filesystem fallback).
fn parse_codex_session(
    rollout_path: &Path,
    codex_processes: &[CodexProcess],
) -> Option<SessionInfo> {
    let metadata = fs::metadata(rollout_path).ok()?;
    let last_modified = metadata.modified().ok()?;
    let last_activity_ms = last_modified
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    let created_at_ms = metadata
        .created()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(last_activity_ms);

    let age = SystemTime::now()
        .duration_since(last_modified)
        .unwrap_or(Duration::from_secs(3600));

    // Skip sessions older than 7 days.
    if age > Duration::from_secs(7 * 24 * 3600) {
        return None;
    }

    let content = read_session_content(rollout_path).ok()?;
    let all_lines: Vec<&str> = content.lines().collect();

    // Parse all lines into JSON values.
    let all_parsed: Vec<Value> = all_lines
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    // Use last N lines for status detection.
    let last_n_start = all_parsed.len().saturating_sub(100);
    let last_n = &all_parsed[last_n_start..];

    // Extract session metadata from the first session_meta line.
    let meta = extract_session_meta(&all_parsed);

    let session_id = meta
        .and_then(|m| m.get("id").and_then(|id| id.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            // Fallback: derive ID from filename.
            rollout_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
                // Strip .jsonl from .jsonl.zst stem
                .strip_suffix(".jsonl")
                .unwrap_or_else(|| {
                    rollout_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("unknown")
                })
                .to_string()
        });

    let workspace_path = meta
        .and_then(|m| m.get("cwd").and_then(|c| c.as_str()))
        .map(|s| s.to_string())
        .unwrap_or_else(|| "codex".to_string());

    let workspace_name = PathBuf::from(&workspace_path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("codex")
        .to_string();

    let agent_nickname = meta
        .and_then(|m| m.get("agent_nickname").and_then(|n| n.as_str()))
        .map(|s| s.to_string());

    // Extract source field for IDE/subagent detection.
    // Source can be a string or JSON object in the rollout meta.
    let source_str = meta
        .and_then(|m| {
            // If source is a string, use it directly.
            // If source is an object, serialize it back for parse_source.
            m.get("source").map(|s| {
                if let Some(str_val) = s.as_str() {
                    str_val.to_string()
                } else {
                    s.to_string()
                }
            })
        })
        .unwrap_or_else(|| "cli".to_string());

    let source_info = parse_source(&source_str);

    let status = determine_status(last_n, age.as_secs_f64());
    let (token_speed, total_output_tokens) = compute_token_stats(&all_parsed);
    let last_message_preview = extract_last_text(last_n);
    let model = extract_model(&all_parsed);
    let context_percent = extract_context_percent(&all_parsed, model.as_deref());
    let (codex_cost, total_input_tokens) = codex_cost_and_input(&all_parsed, model.as_deref());

    // Detect thinking/reasoning.
    let has_reasoning = last_n.iter().any(|v| {
        v.get("type").and_then(|t| t.as_str()) == Some("response_item")
            && v.get("payload")
                .and_then(|p| p.get("type"))
                .and_then(|t| t.as_str())
                == Some("reasoning")
    });
    let thinking_level = if has_reasoning {
        Some("thinking".to_string())
    } else {
        None
    };

    // PID resolution: prefer thread-id match, fall back to workspace path.
    let (pid, pid_precise) = resolve_pid(codex_processes, &session_id, &workspace_path);
    let proc_alive = codex_proc_alive(codex_processes, &session_id);

    // Codex stores only the raw first prompt as its title; for non-subagent
    // threads Fleet generates a semantic title from the first user message
    // (background, cached). Kick that off and prefer any cached result.
    if !source_info.is_subagent {
        if let Some(first_user) = extract_first_user_text(&all_parsed) {
            crate::codex_title::maybe_generate(&session_id, &first_user);
        }
    }

    // Prefer source-embedded nickname > rollout meta nickname
    //      > Fleet-generated semantic title
    let ai_title = source_info
        .agent_nickname
        .or(agent_nickname)
        .or_else(|| crate::codex_title::cached_title(&session_id));

    let agent_type = source_info.agent_role.or_else(|| {
        meta.and_then(|m| m.get("agent_role").and_then(|r| r.as_str()))
            .map(|s| s.to_string())
    });

    // Fleet-ownership marker: rollout `originator` (already in the parsed
    // `session_meta` here, so no extra read). Fleet-launched Codex sessions
    // carry `originator == "fleet"`, recognised by `is_fleet_owned_entrypoint`.
    let entrypoint = meta
        .and_then(|m| m.get("originator").and_then(|o| o.as_str()))
        .filter(|s| !s.is_empty())
        .map(str::to_string);

    // Rate-limit: a Fleet-owned session cut off by the shared account limit is
    // surfaced as RateLimited (with the reset payload) so auto-resume continues
    // it after reset. `codex_rollout_rate_limit` reads the reached window from
    // this session's own rollout — no account→session attribution guessing.
    let rate_limit =
        codex_rollout_rate_limit(&all_parsed, entrypoint.as_deref(), chrono::Utc::now());
    let status = if rate_limit.is_some() {
        crate::session::SessionStatus::RateLimited
    } else {
        status
    };

    let uri = build_uri(rollout_path)?;

    Some(SessionInfo {
        id: session_id,
        workspace_path,
        workspace_name,
        ide_name: source_info.ide_name,
        entrypoint,
        is_subagent: source_info.is_subagent,
        parent_session_id: source_info.parent_thread_id,
        agent_type,
        agent_description: None,
        slug: None,
        ai_title,
        status,
        token_speed,
        agent_token_speed: token_speed,
        total_output_tokens,
        total_input_tokens,
        total_cost_usd: codex_cost,
        agent_total_cost_usd: codex_cost,
        cost_speed_usd_per_min: 0.0,
        last_message_preview,
        last_activity_ms,
        agent_last_activity_ms: last_activity_ms,
        running_subagent_count: 0,
        created_at_ms,
        jsonl_path: uri,
        model,
        thinking_level,
        pid,
        pid_precise,
        // Definitive liveness for Codex: a live process resuming exactly this
        // thread (see `codex_proc_alive`). `apply_pid_liveness` runs only in the
        // Claude scan path, so Codex stamps its own here.
        proc_alive,
        pending_tool_batch: false,
        last_skill: None,
        context_percent,
        agent_source: "codex".to_string(),
        last_outcome: None,
        rate_limit,
        todos: None,
        background_tasks: Vec::new(),
        task_plan: None, handoff: None, user_mark: None, last_read_ms: None,        compact_count: 0,
        compact_pre_tokens: 0,
        compact_post_tokens: 0,
        compact_cost_usd: 0.0,
        pending_messages: Vec::new(),
    })
}

// ── Message normalization ────────────────────────────────────────────────────

/// Normalize Codex rollout lines into Claude-Code-compatible message format.
///
/// Codex format: `{"timestamp":"...","type":"<variant>","payload":{...}}`
/// Fleet format: `{"type":"user|assistant","message":{...},"timestamp":"..."}`
fn normalize_messages(lines: Vec<Value>) -> Vec<Value> {
    // Codex persists every finalized assistant turn TWICE in the rollout:
    //   - as an `event_msg/agent_message` (the UI event-timeline mirror), and
    //   - as a `response_item/message` with role=assistant (the canonical
    //     model-history item — the only copy Codex's own `reconstruct_history_
    //     from_rollout` replays; the event copy is ignored there).
    // Rendering both yields two identical replies, so we suppress the
    // `agent_message` event whenever its text also exists as a response_item
    // assistant message. The event is kept only when no matching response_item
    // is present (e.g. an interrupted or not-yet-persisted turn), so an
    // in-flight reply is never dropped.
    let response_item_assistant_texts: std::collections::HashSet<String> = lines
        .iter()
        .filter_map(|line| {
            if line.get("type").and_then(|t| t.as_str()) != Some("response_item") {
                return None;
            }
            let payload = line.get("payload")?;
            if payload.get("type").and_then(|t| t.as_str()) != Some("message") {
                return None;
            }
            let role = payload
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("assistant");
            if role == "user" || role == "developer" {
                return None;
            }
            let text: String = payload
                .get("content")
                .and_then(|c| c.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| match b.get("type").and_then(|t| t.as_str()) {
                            Some("output_text") | Some("input_text") => {
                                b.get("text").and_then(|t| t.as_str())
                            }
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            if text.is_empty() {
                None
            } else {
                Some(text)
            }
        })
        .collect();

    // Codex also persists each genuine user turn twice: first as the
    // canonical response_item/message(role=user), then as an
    // event_msg/user_message mirror. Keep a count rather than a set so a later
    // interrupted turn that happens to repeat an earlier prompt is not lost.
    let mut response_item_user_texts: std::collections::HashMap<String, usize> =
        std::collections::HashMap::new();
    for line in &lines {
        if line.get("type").and_then(|t| t.as_str()) != Some("response_item") {
            continue;
        }
        let Some(payload) = line.get("payload") else {
            continue;
        };
        if payload.get("type").and_then(|t| t.as_str()) != Some("message")
            || payload.get("role").and_then(|r| r.as_str()) != Some("user")
        {
            continue;
        }
        let text = payload
            .get("content")
            .and_then(|c| c.as_array())
            .map(|blocks| {
                blocks
                    .iter()
                    .filter_map(|b| match b.get("type").and_then(|t| t.as_str()) {
                        Some("input_text") | Some("output_text") => {
                            b.get("text").and_then(|t| t.as_str())
                        }
                        _ => None,
                    })
                    .collect::<Vec<_>>()
                    .join("")
            })
            .unwrap_or_default();
        if !text.is_empty() {
            *response_item_user_texts.entry(text).or_default() += 1;
        }
    }

    let mut messages: Vec<Value> = Vec::new();

    for line in lines {
        let timestamp = line
            .get("timestamp")
            .cloned()
            .unwrap_or(Value::Null);
        let line_type = line
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or_default();
        let payload = match line.get("payload") {
            Some(p) => p,
            None => continue,
        };

        match line_type {
            "event_msg" => {
                let msg_type = payload
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default();

                match msg_type {
                    "user_message" => {
                        let text = payload
                            .get("message")
                            .and_then(|m| m.as_str())
                            .or_else(|| {
                                // Sometimes message is an object with text_elements
                                payload
                                    .get("text_elements")
                                    .and_then(|t| t.as_array())
                                    .and_then(|arr| arr.first())
                                    .and_then(|e| e.as_str())
                            })
                            .unwrap_or("")
                            .to_string();

                        // The response_item is the canonical history record.
                        // Suppress only one matching event mirror per canonical
                        // copy; unmatched events remain visible for partial or
                        // legacy rollouts.
                        if let Some(remaining) = response_item_user_texts.get_mut(&text) {
                            if *remaining > 0 {
                                *remaining -= 1;
                                continue;
                            }
                        }

                        messages.push(json!({
                            "type": "user",
                            "message": {
                                "role": "user",
                                "content": [{"type": "text", "text": text}]
                            },
                            "timestamp": timestamp
                        }));
                    }
                    "agent_message" => {
                        let text = payload
                            .get("message")
                            .and_then(|m| m.as_str())
                            .unwrap_or("")
                            .to_string();

                        // Skip the event-mirror copy when the canonical
                        // response_item carries the same text (see the dedup
                        // note at the top of this fn).
                        if !text.is_empty()
                            && !response_item_assistant_texts.contains(&text)
                        {
                            messages.push(json!({
                                "type": "assistant",
                                "message": {
                                    "role": "assistant",
                                    "content": [{"type": "text", "text": text}],
                                    "stop_reason": "end_turn"
                                },
                                "timestamp": timestamp
                            }));
                        }
                    }
                    // Approval requests — show as system-level waiting messages.
                    "exec_approval_request" | "apply_patch_approval_request"
                    | "mcp_approval_request" => {
                        let desc = match msg_type {
                            "exec_approval_request" => {
                                let cmd = payload
                                    .get("command")
                                    .and_then(|c| c.as_array())
                                    .map(|arr| {
                                        arr.iter()
                                            .filter_map(|v| v.as_str())
                                            .collect::<Vec<_>>()
                                            .join(" ")
                                    })
                                    .or_else(|| {
                                        payload
                                            .get("command")
                                            .and_then(|c| c.as_str())
                                            .map(|s| s.to_string())
                                    })
                                    .unwrap_or_default();
                                format!("⏳ Waiting for approval to execute: {cmd}")
                            }
                            "apply_patch_approval_request" => {
                                let path = payload
                                    .get("path")
                                    .and_then(|p| p.as_str())
                                    .unwrap_or("file");
                                format!("⏳ Waiting for approval to patch: {path}")
                            }
                            _ => "⏳ Waiting for approval".to_string(),
                        };
                        messages.push(json!({
                            "type": "assistant",
                            "message": {
                                "role": "assistant",
                                "content": [{"type": "text", "text": desc}]
                            },
                            "timestamp": timestamp
                        }));
                    }
                    _ => {
                        // Pass through other event_msg types as system messages.
                    }
                }
            }
            "response_item" => {
                let item_type = payload
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or_default();

                match item_type {
                    "message" => {
                        let role = payload
                            .get("role")
                            .and_then(|r| r.as_str())
                            .unwrap_or("assistant");

                        let fleet_type = match role {
                            "user" | "developer" => "user",
                            _ => "assistant",
                        };

                        // Convert content items.
                        let content = payload
                            .get("content")
                            .and_then(|c| c.as_array())
                            .map(|blocks| {
                                blocks
                                    .iter()
                                    .filter_map(|block| {
                                        let bt = block
                                            .get("type")
                                            .and_then(|t| t.as_str())
                                            .unwrap_or_default();
                                        match bt {
                                            "output_text" | "input_text" => {
                                                let text = block
                                                    .get("text")
                                                    .and_then(|t| t.as_str())
                                                    .unwrap_or_default();
                                                Some(json!({"type": "text", "text": text}))
                                            }
                                            _ => None,
                                        }
                                    })
                                    .collect::<Vec<Value>>()
                            })
                            .unwrap_or_default();

                        if !content.is_empty() {
                            let end_turn = payload
                                .get("end_turn")
                                .and_then(|e| e.as_bool())
                                .unwrap_or(false);

                            let mut msg = json!({
                                "type": fleet_type,
                                "message": {
                                    "role": role,
                                    "content": content
                                },
                                "timestamp": timestamp
                            });

                            if end_turn {
                                msg["message"]["stop_reason"] = json!("end_turn");
                            }

                            // Codex injects boilerplate system context as
                            // `role:"developer"` messages (sandbox/permissions
                            // preamble, the `/root` multi-agent collaboration
                            // prompt, `<multi_agent_mode>` guidance). None of it
                            // is user-authored, so tag it `isMeta` — the same
                            // flag Claude Code stamps on harness-injected user
                            // records — and the frontend folds it into a
                            // collapsed, expandable card instead of a full
                            // bubble (see MessageList / SessionDetailView).
                            let text = content
                                .iter()
                                .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
                                .collect::<Vec<_>>()
                                .join("");
                            let is_injected_user_context = role == "user"
                                && text.trim_start().starts_with("<recommended_plugins>")
                                && text.contains("</recommended_plugins>");

                            if role == "developer" || is_injected_user_context {
                                msg["isMeta"] = json!(true);
                            }

                            // Attach ID if present.
                            if let Some(id) = payload.get("id") {
                                msg["message"]["id"] = id.clone();
                            }

                            messages.push(msg);
                        }
                    }
                    "reasoning" => {
                        // Convert reasoning to a thinking block in an assistant message.
                        let summary_text = payload
                            .get("summary")
                            .and_then(|s| s.as_array())
                            .and_then(|arr| {
                                arr.iter()
                                    .filter_map(|item| {
                                        item.get("text").and_then(|t| t.as_str())
                                    })
                                    .next()
                            })
                            .unwrap_or("(reasoning)");

                        messages.push(json!({
                            "type": "assistant",
                            "message": {
                                "role": "assistant",
                                "content": [{
                                    "type": "thinking",
                                    "thinking": summary_text
                                }]
                            },
                            "timestamp": timestamp
                        }));
                    }
                    "local_shell_call" => {
                        let action = payload.get("action");
                        let command = action
                            .and_then(|a| a.get("command"))
                            .and_then(|c| c.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str())
                                    .collect::<Vec<_>>()
                                    .join(" ")
                            })
                            .unwrap_or_default();
                        let status_str = payload
                            .get("status")
                            .and_then(|s| s.as_str())
                            .unwrap_or("completed");
                        let output = action
                            .and_then(|a| a.get("output"))
                            .and_then(|o| o.as_str())
                            .unwrap_or_default();
                        let call_id = payload
                            .get("call_id")
                            .and_then(|c| c.as_str())
                            .or_else(|| payload.get("id").and_then(|i| i.as_str()))
                            .unwrap_or("shell");

                        // Emit as tool_use + tool_result pair.
                        messages.push(json!({
                            "type": "assistant",
                            "message": {
                                "role": "assistant",
                                "content": [{
                                    "type": "tool_use",
                                    "id": call_id,
                                    "name": "Bash",
                                    "input": {"command": command}
                                }]
                            },
                            "timestamp": timestamp
                        }));

                        if status_str == "completed" {
                            messages.push(json!({
                                "type": "tool_result",
                                "tool_use_id": call_id,
                                "content": output,
                                "timestamp": timestamp
                            }));
                        }
                    }
                    "function_call" => {
                        let name = payload
                            .get("name")
                            .and_then(|n| n.as_str())
                            .unwrap_or("unknown");
                        let arguments = payload
                            .get("arguments")
                            .and_then(|a| a.as_str())
                            .unwrap_or("{}");
                        let call_id = payload
                            .get("call_id")
                            .and_then(|c| c.as_str())
                            .unwrap_or("fn");

                        // Try to parse arguments as JSON for display.
                        let input: Value =
                            serde_json::from_str(arguments).unwrap_or(json!({"raw": arguments}));

                        messages.push(json!({
                            "type": "assistant",
                            "message": {
                                "role": "assistant",
                                "content": [{
                                    "type": "tool_use",
                                    "id": call_id,
                                    "name": name,
                                    "input": input
                                }]
                            },
                            "timestamp": timestamp
                        }));
                    }
                    "function_call_output" => {
                        let call_id = payload
                            .get("call_id")
                            .and_then(|c| c.as_str())
                            .unwrap_or("fn");
                        let output = payload
                            .get("output")
                            .and_then(|o| {
                                o.get("text")
                                    .and_then(|t| t.as_str())
                                    .or_else(|| o.as_str())
                            })
                            .unwrap_or("");

                        messages.push(json!({
                            "type": "tool_result",
                            "tool_use_id": call_id,
                            "content": output,
                            "timestamp": timestamp
                        }));
                    }
                    "web_search_call" => {
                        let query = payload
                            .get("query")
                            .and_then(|q| q.as_str())
                            .or_else(|| {
                                payload
                                    .get("action")
                                    .and_then(|a| a.get("query"))
                                    .and_then(|q| q.as_str())
                            })
                            .unwrap_or("(web search)");
                        let call_id = payload
                            .get("call_id")
                            .and_then(|c| c.as_str())
                            .or_else(|| payload.get("id").and_then(|i| i.as_str()))
                            .unwrap_or("web_search");
                        let status_str = payload
                            .get("status")
                            .and_then(|s| s.as_str())
                            .unwrap_or("completed");

                        messages.push(json!({
                            "type": "assistant",
                            "message": {
                                "role": "assistant",
                                "content": [{
                                    "type": "tool_use",
                                    "id": call_id,
                                    "name": "WebSearch",
                                    "input": {"query": query}
                                }]
                            },
                            "timestamp": timestamp
                        }));

                        if status_str == "completed" {
                            let output = payload
                                .get("action")
                                .and_then(|a| a.get("output"))
                                .and_then(|o| o.as_str())
                                .unwrap_or("(search completed)");
                            messages.push(json!({
                                "type": "tool_result",
                                "tool_use_id": call_id,
                                "content": output,
                                "timestamp": timestamp
                            }));
                        }
                    }
                    "image_generation_call" => {
                        let prompt = payload
                            .get("action")
                            .and_then(|a| a.get("prompt"))
                            .and_then(|p| p.as_str())
                            .unwrap_or("(image generation)");
                        let call_id = payload
                            .get("call_id")
                            .and_then(|c| c.as_str())
                            .or_else(|| payload.get("id").and_then(|i| i.as_str()))
                            .unwrap_or("image_gen");

                        messages.push(json!({
                            "type": "assistant",
                            "message": {
                                "role": "assistant",
                                "content": [{
                                    "type": "tool_use",
                                    "id": call_id,
                                    "name": "ImageGeneration",
                                    "input": {"prompt": prompt}
                                }]
                            },
                            "timestamp": timestamp
                        }));
                    }
                    "compaction" => {
                        // Show context compaction as an informational message.
                        messages.push(json!({
                            "type": "assistant",
                            "message": {
                                "role": "assistant",
                                "content": [{"type": "text", "text": "📦 Context compacted"}]
                            },
                            "timestamp": timestamp
                        }));
                    }
                    _ => {
                        // Skip other response_item types (ghost_snapshot, etc.)
                    }
                }
            }
            _ => {
                // Skip session_meta, turn_context, compacted, etc.
            }
        }
    }

    messages
}

// ── AgentSource implementation ──────────────────────────────────────────────

impl AgentSource for CodexSource {
    fn name(&self) -> &'static str {
        "codex"
    }

    fn uri_prefix(&self) -> &'static str {
        CODEX_URI_PREFIX
    }

    fn is_available(&self) -> bool {
        get_codex_dir().map(|d| d.is_dir()).unwrap_or(false)
    }

    fn scan_sessions(&self) -> Vec<SessionInfo> {
        // Reuse cached process list if fresh (< 10 s).
        let codex_processes = {
            let mut guard = self.process_cache.lock().unwrap();
            let stale = guard.0.map_or(true, |t| t.elapsed() > Duration::from_secs(10));
            if stale {
                guard.1 = scan_codex_processes();
                guard.0 = Some(std::time::Instant::now());
            }
            guard.1.clone()
        };

        // Try SQLite first (fast path).
        if let Some(sessions) = self.scan_from_sqlite(&codex_processes) {
            return sessions;
        }

        // Fallback: filesystem scan.
        self.scan_from_filesystem(&codex_processes)
    }

    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String> {
        let file_path =
            resolve_uri(path).ok_or_else(|| format!("Invalid Codex URI: {path}"))?;

        let content = read_session_content(&file_path)?;

        let parsed: Vec<Value> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect();

        Ok(normalize_messages(parsed))
    }

    fn get_messages_tail(&self, path: &str, n: usize) -> Result<Vec<Value>, String> {
        let file_path =
            resolve_uri(path).ok_or_else(|| format!("Invalid Codex URI: {path}"))?;
        let name = file_path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or_default();

        // Plain `.jsonl` can use the byte-level reverse-scan tail reader. The
        // zstd-compressed variant must be fully decompressed first; in that
        // case we still avoid parsing every line by slicing to the last n
        // before `serde_json::from_str`.
        let parsed: Vec<Value> = if name.ends_with(".jsonl.zst") {
            let content = read_zst_file(&file_path)?;
            let lines: Vec<&str> = content
                .lines()
                .filter(|l| !l.trim().is_empty())
                .collect();
            let start = lines.len().saturating_sub(n);
            lines[start..]
                .iter()
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect()
        } else {
            crate::jsonl_tail::read_tail_lines_as_json(&file_path, n)
                .map_err(|e| e.to_string())?
        };

        Ok(normalize_messages(parsed))
    }

    fn resolve_file_path(&self, path: &str) -> Option<PathBuf> {
        resolve_uri(path)
    }

    fn watch_strategy(&self) -> WatchStrategy {
        WatchStrategy::Filesystem
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        get_sessions_dir()
            .into_iter()
            .filter(|d| d.is_dir())
            .collect()
    }

    fn trigger_extensions(&self) -> Vec<&'static str> {
        vec!["zst", "jsonl"]
    }

    fn kill_pid(&self, pid: u32) -> Result<(), String> {
        crate::session::kill_pid_impl(pid)
    }

    fn fetch_account(&self) -> Result<Value, String> {
        Err("Codex does not have a separate account endpoint; plan info is included in usage".into())
    }

    fn fetch_usage(&self) -> Result<Value, String> {
        let info = fetch_codex_usage_blocking()?;
        serde_json::to_value(&info).map_err(|e| e.to_string())
    }

    fn usage_summary(&self) -> Option<SourceUsageSummary> {
        let info = fetch_codex_usage_blocking().ok()?;
        let val = serde_json::to_value(&info).ok()?;
        Some(SourceUsageSummary::from_codex(&val))
    }

    fn spawn(
        &self,
        spec: &crate::agent_source::SpawnSpec,
    ) -> Result<crate::session_launch::SpawnSessionResponse, String> {
        // Codex mints its own thread id (no --session-id) and has no entrypoint
        // env; session_id/entrypoint on the spec are ignored here. The
        // permission_mode → sandbox/approval mapping is a later milestone.
        crate::codex_launch::spawn_new_codex_session(
            &spec.workspace_path,
            &spec.prompt,
            spec.model.as_deref(),
            spec.effort.as_deref(),
        )
    }

    fn resume(
        &self,
        spec: &crate::agent_source::ResumeSpec,
        on_exit: Box<dyn FnOnce(bool) + Send>,
    ) -> Result<(), String> {
        // permission_mode is Claude's --permission-mode; Codex's sandbox/approval
        // mapping is a later milestone, so it's ignored here.
        crate::codex_launch::resume_codex_session(
            &spec.session_id,
            &spec.workspace_path,
            &spec.prompt,
            spec.model.as_deref(),
            spec.effort.as_deref(),
            on_exit,
        )
    }
}

// ── Codex usage via app-server protocol ──────────────────────────────────────

/// Rate-limit window from the Codex app-server `account/rateLimits/read` response.
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodexRateLimitWindow {
    pub used_percent: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_duration_mins: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

/// Credits snapshot from Codex.
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodexCreditsSnapshot {
    pub has_credits: bool,
    pub unlimited: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<String>,
}

/// Full rate-limit snapshot returned to the frontend.
#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CodexUsageItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub primary: Option<CodexRateLimitWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub secondary: Option<CodexRateLimitWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub credits: Option<CodexCreditsSnapshot>,
    /// Which window (if any) the account has actually hit: `"primary"` /
    /// `"secondary"`, or `None`/absent when not currently rate-limited. This is
    /// the authoritative "am I limited right now" signal from the app-server
    /// (per `account/rateLimits/read`), used to decide codex auto-resume
    /// eligibility rather than guessing from a per-session transcript line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rate_limit_reached_type: Option<String>,
}

/// Build a Claude-shaped [`crate::session::RateLimitState`] from a Codex
/// account rate-limit snapshot, for feeding [`crate::auto_resume::should_auto_resume`].
///
/// Returns `None` unless the snapshot reports a window has actually been hit
/// (`rate_limit_reached_type` set) — Fleet only auto-resumes a codex session
/// that is genuinely limited.
///
/// Codex-specific semantics (all verified against codex-cli 0.144.4):
/// - `resetsAt` is Unix epoch **seconds** (Claude's parsed reset is a
///   `DateTime`), so it is converted with `from_timestamp(secs, 0)`.
/// - `error_timestamp` is set to `resets_at − windowDuration`, i.e. the window
///   *start*, NOT `now`. This makes `wait = resets_at − error_timestamp` equal
///   the window length, so the `max_wait_hours` safety valve treats a codex
///   **weekly** (10080-min) limit exactly like a Claude weekly limit — too long
///   to auto-resume unattended — while a short **secondary** window (e.g. 5h)
///   stays eligible once it resets. Unknown window duration → 7 days
///   (conservatively blocked).
/// - `limit_type` is `Unknown` so the Claude-metric recovery path
///   ([`crate::auto_resume::limit_recovered`]) is skipped (codex has no
///   five_hour/seven_day sample); recovery is purely the hinted `resetsAt` +
///   grace.
pub fn codex_rate_limit_state_from_usage(
    usage: &CodexUsageItem,
    _now: chrono::DateTime<chrono::Utc>,
) -> Option<crate::session::RateLimitState> {
    let reached = usage.rate_limit_reached_type.as_deref()?;
    let window = match reached {
        "secondary" => usage.secondary.as_ref().or(usage.primary.as_ref()),
        // "primary" or any other non-empty marker → the primary window.
        _ => usage.primary.as_ref(),
    }?;
    let resets_at_secs = window.resets_at?;
    let resets_at = chrono::DateTime::<chrono::Utc>::from_timestamp(resets_at_secs, 0)?;
    let window_dur = match window.window_duration_mins {
        Some(mins) if mins > 0 => chrono::Duration::minutes(mins),
        _ => chrono::Duration::days(7),
    };
    Some(crate::session::RateLimitState {
        resets_at,
        limit_type: crate::rate_limit_parser::RateLimitType::Unknown,
        parsed: true,
        error_timestamp: resets_at - window_dur,
    })
}

/// Build a [`crate::session::RateLimitState`] from a Codex session's **own
/// rollout** `token_count.rate_limits` object — the per-session view of the
/// shared account limit that Codex embeds in every `token_count` event.
///
/// The rollout shape differs from the app-server `account/rateLimits/read`
/// snapshot [`codex_rate_limit_state_from_usage`] consumes: it is snake_case and
/// names the window length `window_minutes` (the app-server spells it
/// `windowDurationMins`). We normalise it into a [`CodexUsageItem`] and delegate,
/// so the reset judgment (window pick, `resets_at`, `error_timestamp`,
/// `max_wait` semantics) lives in exactly one place.
///
/// Keyed off `rate_limit_reached_type` exactly like the app-server path
/// (verified against real rollouts: healthy turns carry
/// `rate_limit_reached_type: null`, older pre-0.14x rollouts omit the field
/// entirely). Both non-limited cases yield `None`; a genuinely rate-limited turn
/// carries `"primary"`/`"secondary"` and yields `Some`.
pub fn codex_rate_limit_state_from_rollout(
    rate_limits: &Value,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<crate::session::RateLimitState> {
    // Absent or null reached-type → not limited (or a legacy rollout without
    // the field). `.as_str()` on JSON null returns None, covering both.
    let reached = rate_limits.get("rate_limit_reached_type")?.as_str()?.to_string();
    let window_from = |key: &str| -> Option<CodexRateLimitWindow> {
        let w = rate_limits.get(key)?;
        if w.is_null() {
            return None;
        }
        Some(CodexRateLimitWindow {
            used_percent: w.get("used_percent").and_then(|v| v.as_f64()).unwrap_or(0.0) as i32,
            // Rollout names it `window_minutes`; the app-server struct field is
            // `window_duration_mins` (camelCase `windowDurationMins`).
            window_duration_mins: w.get("window_minutes").and_then(|v| v.as_i64()),
            resets_at: w.get("resets_at").and_then(|v| v.as_i64()),
        })
    };
    let usage = CodexUsageItem {
        primary: window_from("primary"),
        secondary: window_from("secondary"),
        rate_limit_reached_type: Some(reached),
        ..Default::default()
    };
    codex_rate_limit_state_from_usage(&usage, now)
}

/// Find the most recent `token_count.rate_limits` object in a Codex rollout —
/// the session's latest view of the shared account limit (Codex embeds it in
/// every `token_count` event).
fn last_rollout_rate_limits(parsed: &[Value]) -> Option<&Value> {
    parsed.iter().rev().find_map(|v| {
        let p = v.get("payload")?;
        if p.get("type")?.as_str()? != "token_count" {
            return None;
        }
        p.get("rate_limits").filter(|rl| !rl.is_null())
    })
}

/// Whether a Codex rollout ends WITHOUT a clean turn completion — the analogue
/// of "the turn was cut off". A session that finished normally (last turn
/// boundary is `turn_complete`/`task_complete`) is NOT treated as rate-limited
/// even if its last usage snapshot happens to report a reached window; only a
/// session that stopped mid-work (last boundary is
/// `turn_started`/`turn_aborted`/`error`) is a resume candidate.
fn codex_last_turn_incomplete(parsed: &[Value]) -> bool {
    for v in parsed.iter().rev() {
        if v.get("type").and_then(|t| t.as_str()) != Some("event_msg") {
            continue;
        }
        match v
            .get("payload")
            .and_then(|p| p.get("type"))
            .and_then(|t| t.as_str())
        {
            Some("turn_complete") | Some("task_complete") => return false,
            Some("turn_started") | Some("turn_aborted") | Some("error") => return true,
            _ => {}
        }
    }
    false
}

/// Decide the rate-limit state for a Codex session from its **own rollout**, for
/// [`crate::auto_resume`]. Returns `Some` only when ALL hold:
/// - the session is Fleet-owned (`originator == "fleet"`) — Fleet only resumes
///   sessions it launched, matching the Claude auto-resume gate;
/// - the last turn did not complete cleanly ([`codex_last_turn_incomplete`]);
/// - the latest `token_count.rate_limits` reports a reached window
///   ([`codex_rate_limit_state_from_rollout`]).
///
/// The auto-resume scheduler's own guards (reset grace, `max_wait_hours`,
/// backoff, concurrency cap) bound what actually gets re-spawned, so a
/// false-positive here is capped rather than a runaway.
fn codex_rollout_rate_limit(
    parsed: &[Value],
    entrypoint: Option<&str>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<crate::session::RateLimitState> {
    if !crate::session_launch::is_fleet_owned_entrypoint(entrypoint) {
        return None;
    }
    if !codex_last_turn_incomplete(parsed) {
        return None;
    }
    codex_rate_limit_state_from_rollout(last_rollout_rate_limits(parsed)?, now)
}

/// Locate a runnable Codex binary.
///
/// Search order (first hit wins):
///   1. The **standalone auto-update install** at
///      `<CODEX_HOME>/packages/standalone/current/bin/codex` — the binary the
///      `codex` CLI's own updater manages and the one the interactive TUI runs.
///      Its `current` symlink always points at the newest installed release.
///   2. The Codex binary shipped inside the OpenAI ChatGPT VSCode extension
///      (`~/.vscode/extensions/openai.chatgpt-*/bin/<platform>/codex`), newest
///      version first.
///   3. Bare `codex` on `PATH`.
///
/// The standalone install is tried first deliberately: on machines where the
/// only thing on `PATH` is a broken npm/homebrew `codex` wrapper (a JS shim
/// whose vendored binary is missing → `ENOENT` on exec), the `which codex`
/// fallback would hand back that broken shim. The standalone `current` binary
/// is a real executable, so preferring it fixes both the usage probe and
/// (P1+) session spawning without a per-call liveness check.
///
/// Shared by the rate-limit usage probe and the Codex session launcher
/// (`codex_launch`), so both agree on which binary to run.
pub fn find_codex_binary() -> Option<std::path::PathBuf> {
    let home = crate::session::real_home_dir()?;

    #[cfg(windows)]
    let exe = "codex.exe";
    #[cfg(not(windows))]
    let exe = "codex";

    // 1. Standalone auto-update install (`current` → newest release).
    if let Some(codex_dir) = get_codex_dir() {
        let standalone = codex_dir
            .join("packages")
            .join("standalone")
            .join("current")
            .join("bin")
            .join(exe);
        if standalone.exists() {
            return Some(standalone);
        }
    }

    // 2. Check VSCode extension directories
    let ext_dirs = [
        home.join(".vscode").join("extensions"),
        home.join(".vscode-insiders").join("extensions"),
    ];

    #[cfg(target_os = "macos")]
    let bin_subpath = "bin/macos-aarch64/codex";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    let bin_subpath = "bin/linux-x64/codex";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    let bin_subpath = "bin/linux-arm64/codex";
    #[cfg(target_os = "windows")]
    let bin_subpath = "bin/windows-x64/codex.exe";

    for ext_dir in &ext_dirs {
        if let Ok(entries) = std::fs::read_dir(ext_dir) {
            // Collect matching extension dirs and pick the latest version
            let mut candidates: Vec<std::path::PathBuf> = entries
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.file_name()
                        .to_str()
                        .map_or(false, |n| n.starts_with("openai.chatgpt-"))
                })
                .map(|e| e.path().join(bin_subpath))
                .filter(|p| p.exists())
                .collect();
            // Sort descending so the newest version comes first
            candidates.sort_by(|a, b| b.cmp(a));
            if let Some(bin) = candidates.into_iter().next() {
                return Some(bin);
            }
        }
    }

    // Fallback: `codex` in PATH
    #[cfg(unix)]
    {
        if std::process::Command::new("which")
            .arg("codex")
            .output()
            .map_or(false, |o| o.status.success())
        {
            return Some(std::path::PathBuf::from("codex"));
        }
    }

    None
}

/// Query rate limits from the Codex app-server via its JSON-RPC stdio protocol.
///
/// Spawns a short-lived `codex app-server` process, sends `initialize` +
/// `account/rateLimits/read`, and returns the snapshot.
pub async fn fetch_codex_usage() -> Result<CodexUsageItem, String> {
    let bin = find_codex_binary().ok_or("Codex binary not found")?;

    // Run blocking I/O in a background thread to avoid blocking the async runtime.
    let result: Result<CodexUsageItem, String> =
        tokio::task::spawn_blocking(move || fetch_codex_usage_blocking_impl(&bin))
            .await
            .map_err(|e| format!("join error: {e}"))?;
    result
}

/// Blocking wrapper for use in fleet CLI (no tauri async runtime).
pub fn fetch_codex_usage_blocking() -> Result<CodexUsageItem, String> {
    let bin = find_codex_binary().ok_or("Codex binary not found")?;
    fetch_codex_usage_blocking_impl(&bin)
}

/// Blocking implementation of the Codex app-server query.
fn fetch_codex_usage_blocking_impl(bin: &std::path::Path) -> Result<CodexUsageItem, String> {
    use std::io::{BufRead, BufReader, Write};

    let mut child = crate::process_util::command(bin)
        .arg("app-server")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("Failed to spawn codex app-server: {e}"))?;

    let mut stdin = child.stdin.take().ok_or("No stdin")?;
    let stdout = child.stdout.take().ok_or("No stdout")?;
    let mut reader = BufReader::new(stdout);

    let send = |stdin: &mut std::process::ChildStdin, msg: &serde_json::Value| -> Result<(), String> {
        let mut data = serde_json::to_vec(msg).unwrap();
        data.push(b'\n');
        stdin.write_all(&data).map_err(|e| format!("write: {e}"))
    };

    // 1. initialize
    send(&mut stdin, &serde_json::json!({
        "jsonrpc": "2.0", "id": 1,
        "method": "initialize",
        "params": {
            "clientInfo": { "name": "fleet", "version": "0.1" },
            "capabilities": { "experimentalApi": true }
        }
    }))?;

    // Read init response (discard)
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|e| format!("read init: {e}"))?;

    // 2. initialized notification
    send(&mut stdin, &serde_json::json!({"jsonrpc":"2.0","method":"initialized"}))?;

    // 3. account/rateLimits/read
    send(&mut stdin, &serde_json::json!({
        "jsonrpc": "2.0", "id": 2,
        "method": "account/rateLimits/read",
        "params": {}
    }))?;

    // Read lines until we get the response with id=2 (timeout via child kill after 10s).
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if std::time::Instant::now() > deadline {
            let _ = child.kill();
            return Err("Timeout waiting for rate-limit response".to_string());
        }

        let mut resp_line = String::new();
        match reader.read_line(&mut resp_line) {
            Ok(0) => {
                let _ = child.kill();
                return Err("EOF waiting for rate-limit response".to_string());
            }
            Err(e) => {
                let _ = child.kill();
                return Err(format!("read error: {e}"));
            }
            Ok(_) => {
                if let Ok(msg) = serde_json::from_str::<serde_json::Value>(resp_line.trim()) {
                    if msg.get("id").and_then(|v| v.as_i64()) == Some(2) {
                        let _ = child.kill();
                        if let Some(err) = msg.get("error") {
                            return Err(format!(
                                "Codex error: {}",
                                err.get("message")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("unknown")
                            ));
                        }
                        let result = msg
                            .get("result")
                            .ok_or("Missing result in response")?;
                        let snapshot: CodexUsageItem = serde_json::from_value(
                            result
                                .get("rateLimits")
                                .cloned()
                                .unwrap_or_else(|| result.clone()),
                        )
                        .map_err(|e| format!("parse rate-limit: {e}"))?;
                        return Ok(snapshot);
                    }
                }
            }
        }
    }
}

impl CodexSource {
    /// Scan sessions using SQLite metadata (fast path).
    fn scan_from_sqlite(&self, codex_processes: &[CodexProcess]) -> Option<Vec<SessionInfo>> {
        let threads = read_threads_from_sqlite()?;

        let mut sessions = Vec::new();
        for thread in &threads {
            if let Some(info) = build_session_from_sqlite(thread, codex_processes) {
                sessions.push(info);
            }
        }

        Some(sessions)
    }

    /// Scan sessions by walking the filesystem (fallback when SQLite is unavailable).
    fn scan_from_filesystem(&self, codex_processes: &[CodexProcess]) -> Vec<SessionInfo> {
        let Some(sessions_dir) = get_sessions_dir() else {
            return vec![];
        };
        if !sessions_dir.is_dir() {
            return vec![];
        }

        let rollout_files = find_rollout_files(&sessions_dir);
        let mut sessions = Vec::new();
        let mut seen_ids: HashSet<String> = HashSet::new();

        // Process newest files first (they are more likely to be relevant).
        let mut files_with_mtime: Vec<_> = rollout_files
            .into_iter()
            .filter_map(|p| {
                let mtime = fs::metadata(&p).ok()?.modified().ok()?;
                Some((p, mtime))
            })
            .collect();
        files_with_mtime.sort_by(|a, b| b.1.cmp(&a.1));

        for (path, _) in files_with_mtime {
            if let Some(info) = parse_codex_session(&path, codex_processes) {
                if seen_ids.insert(info.id.clone()) {
                    sessions.push(info);
                }
            }
        }

        sessions
    }
}
