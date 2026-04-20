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
    process_cache: std::sync::Mutex<(std::time::Instant, Vec<CodexProcess>)>,
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
            process_cache: std::sync::Mutex::new((
                std::time::Instant::now() - std::time::Duration::from_secs(999),
                Vec::new(),
            )),
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
        fs::read_to_string(path).map_err(|e| format!("Cannot read {}: {e}", path.display()))
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
        let usage = info
            .get("total_token_usage")
            .or_else(|| info.get("last_token_usage"));

        let input_tokens = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cached_input_tokens = usage
            .and_then(|u| u.get("cached_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let total_input = input_tokens + cached_input_tokens;

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
            "--thread" | "--resume" | "-t" => {
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
    let is_subagent = matches!(
        source.to_lowercase().as_str(),
        "sub_agent" | "subagent" | "exec"
    );

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
    let (status, token_speed, total_output_tokens, last_message_preview, model, thinking_level, context_percent) =
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
                        (st, spd, tok, preview, mdl, tl, ctx)
                    } else {
                        (
                            determine_status_from_age(file_age.as_secs_f64()),
                            0.0,
                            thread.tokens_used as u64,
                            None,
                            thread.model.clone(),
                            None,
                            None,
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

    // Prefer: source-embedded nickname > SQLite agent_nickname > title
    let ai_title = source_info
        .agent_nickname
        .or_else(|| thread.agent_nickname.clone())
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

    let uri = build_uri(&rollout_path)?;

    Some(SessionInfo {
        id: thread.id.clone(),
        workspace_path: thread.cwd.clone(),
        workspace_name,
        ide_name: source_info.ide_name,
        is_subagent: source_info.is_subagent,
        parent_session_id: source_info.parent_thread_id,
        agent_type,
        agent_description: None,
        slug: None,
        ai_title,
        status,
        token_speed,
        total_output_tokens,
        total_cost_usd: 0.0,
        agent_total_cost_usd: 0.0,
        cost_speed_usd_per_min: 0.0,
        last_message_preview,
        last_activity_ms,
        created_at_ms,
        jsonl_path: uri,
        model,
        thinking_level,
        pid,
        pid_precise,
        last_skill: None,
        context_percent,
        agent_source: "codex".to_string(),
        last_outcome: None,
        rate_limit: None,
        todos: None,
    })
}

#[cfg(test)]
mod tests {
    use super::{compute_token_stats, extract_context_percent};
    use serde_json::json;

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
        let lines = vec![json!({
            "type": "event_msg",
            "payload": {
                "type": "token_count",
                "info": {
                    "total_token_usage": {
                        "input_tokens": 1000,
                        "cached_input_tokens": 500
                    },
                    "model_context_window": 3000
                }
            }
        })];

        let pct = extract_context_percent(&lines, Some("gpt-5.4"));
        assert_eq!(pct, Some(0.5));
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

    // Prefer source-embedded nickname > rollout meta nickname
    let ai_title = source_info
        .agent_nickname
        .or(agent_nickname);

    let agent_type = source_info.agent_role.or_else(|| {
        meta.and_then(|m| m.get("agent_role").and_then(|r| r.as_str()))
            .map(|s| s.to_string())
    });

    let uri = build_uri(rollout_path)?;

    Some(SessionInfo {
        id: session_id,
        workspace_path,
        workspace_name,
        ide_name: source_info.ide_name,
        is_subagent: source_info.is_subagent,
        parent_session_id: source_info.parent_thread_id,
        agent_type,
        agent_description: None,
        slug: None,
        ai_title,
        status,
        token_speed,
        total_output_tokens,
        total_cost_usd: 0.0,
        agent_total_cost_usd: 0.0,
        cost_speed_usd_per_min: 0.0,
        last_message_preview,
        last_activity_ms,
        created_at_ms,
        jsonl_path: uri,
        model,
        thinking_level,
        pid,
        pid_precise,
        last_skill: None,
        context_percent,
        agent_source: "codex".to_string(),
        last_outcome: None,
        rate_limit: None,
        todos: None,
    })
}

// ── Message normalization ────────────────────────────────────────────────────

/// Normalize Codex rollout lines into Claude-Code-compatible message format.
///
/// Codex format: `{"timestamp":"...","type":"<variant>","payload":{...}}`
/// Fleet format: `{"type":"user|assistant","message":{...},"timestamp":"..."}`
fn normalize_messages(lines: Vec<Value>) -> Vec<Value> {
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

                        if !text.is_empty() {
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
            if guard.0.elapsed() > Duration::from_secs(10) {
                guard.1 = scan_codex_processes();
                guard.0 = std::time::Instant::now();
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
}

/// Locate the Codex binary shipped inside the OpenAI ChatGPT VSCode/Cursor extension.
fn find_codex_binary() -> Option<std::path::PathBuf> {
    let home = crate::session::real_home_dir()?;

    // Check Cursor and VSCode extension directories
    let ext_dirs = [
        home.join(".cursor").join("extensions"),
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

    let mut child = std::process::Command::new(bin)
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
