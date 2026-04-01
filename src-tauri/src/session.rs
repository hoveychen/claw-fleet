use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::hooks::HookState;

// ── Lock file ────────────────────────────────────────────────────────────────

#[derive(Deserialize, Clone, Debug)]
pub struct LockFile {
    pub pid: u32,
    #[serde(rename = "workspaceFolders", default)]
    pub workspace_folders: Vec<String>,
    #[serde(rename = "ideName", default)]
    pub ide_name: String,
}

pub struct IdeSession {
    pub pid: u32,
    pub workspace_folders: Vec<String>,
    pub ide_name: String,
}

// ── Exported types ───────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Thinking,     // streaming: last partial assistant msg has thinking blocks
    Executing,    // streaming: last partial assistant msg has tool_use blocks
    Streaming,    // file written < 2s ago (text output)
    Delegating,   // main session with at least one active subagent
    Processing,   // last stop_reason = tool_use, recent activity (waiting for tool result)
    WaitingInput, // last stop_reason = end_turn
    Active,       // file written < 30s ago
    Idle,         // no recent activity
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: String,
    pub workspace_path: String,
    pub workspace_name: String,
    pub ide_name: Option<String>,
    pub is_subagent: bool,
    pub parent_session_id: Option<String>,
    pub agent_type: Option<String>,
    pub agent_description: Option<String>,
    pub slug: Option<String>,
    pub ai_title: Option<String>,
    pub status: SessionStatus,
    pub token_speed: f64,
    pub total_output_tokens: u64,
    pub last_message_preview: Option<String>,
    pub last_activity_ms: u64,
    pub created_at_ms: u64,
    pub jsonl_path: String,
    pub model: Option<String>,
    pub thinking_level: Option<String>,
    pub pid: Option<u32>,
    /// True when the PID is unambiguously matched to this specific session.
    /// False when multiple claude processes share the same cwd and none carries
    /// a matching --resume flag — stopping may affect sibling sessions.
    pub pid_precise: bool,
    pub last_skill: Option<String>,
    /// Approximate context-window utilisation (0.0 – 1.0) derived from the
    /// last finalized assistant message's usage fields.  `None` when no
    /// usage data is available.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<f64>,
    /// Source of this session: "claude-code" or "cursor"
    pub agent_source: String,
    /// Semantic outcome tags from the last completed turn (e.g. "bug_fixed",
    /// "needs_input").  Set by background analysis, cleared when a new turn
    /// starts.  `None` means no analysis has run yet or the session is busy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_outcome: Option<Vec<String>>,
}

// ── Helpers ──────────────────────────────────────────────────────────────────

pub fn get_claude_dir() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".claude"))
}

#[cfg(unix)]
pub fn is_process_alive(pid: u32) -> bool {
    let ret = unsafe { libc::kill(pid as libc::pid_t, 0) };
    if ret == 0 {
        return true;
    }
    // EPERM = process exists but we lack permission to signal it → alive
    // ESRCH = no such process → dead
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

#[cfg(not(unix))]
pub fn is_process_alive(_pid: u32) -> bool {
    true // Fallback: assume alive
}

fn decode_workspace_path(encoded: &str) -> String {
    // "-Users-hoveychen-workspace-netferry" → "/Users/hoveychen/workspace/netferry"
    //
    // Dashes are ambiguous (path separator vs literal dash in a directory name).
    // We resolve this by greedily checking the filesystem: at each level, try the
    // longest remaining dash-joined segment first, and shorten until we find a real
    // directory.  Fall back to the naive one-dash-per-slash decode if nothing matches.
    let stripped = encoded.trim_start_matches('-');
    let parts: Vec<&str> = stripped.split('-').collect();
    if parts.is_empty() {
        return "/".to_string();
    }
    decode_workspace_path_with_parts(&parts)
}

pub fn decode_workspace_path_with_parts(parts: &[&str]) -> String {
    let mut current = String::new(); // built path so far (e.g. "/Users/hoveychen")
    let mut i = 0;
    while i < parts.len() {
        // Try longest remaining segment first: join parts[i..] with '-', then parts[i..len-1], etc.
        let mut matched = false;
        // Try from longest (all remaining parts) down to single part
        for end in (i + 1..=parts.len()).rev() {
            let candidate_segment = parts[i..end].join("-");
            let candidate_path = format!("{}/{}", current, candidate_segment);
            if std::path::Path::new(&candidate_path).exists() {
                current = candidate_path;
                i = end;
                matched = true;
                break;
            }
        }
        if !matched {
            // Nothing exists on disk — use single part (original naive behavior)
            current = format!("{}/{}", current, parts[i]);
            i += 1;
        }
    }
    current
}

fn encode_workspace_path(path: &str) -> String {
    // "/Users/foo/bar-baz" → "-Users-foo-bar-baz"  (inverse of decode, but lossless for matching)
    path.replace('/', "-")
}

fn workspace_name(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(path)
        .to_string()
}

// ── CLI process scanning ─────────────────────────────────────────────────────

/// A running `claude` process discovered by sysinfo.
#[derive(Debug, Clone)]
pub struct CliProcess {
    pub pid: u32,
    pub ppid: Option<u32>,
    pub cwd: String,
    /// Session ID parsed from `--resume <id>` in the process argv, if present.
    pub resume_session_id: Option<String>,
}

fn extract_resume_id(cmd: &[std::ffi::OsString]) -> Option<String> {
    let mut iter = cmd.iter();
    while let Some(arg) = iter.next() {
        let s = arg.to_string_lossy();
        if s == "--resume" || s == "-r" {
            return iter.next().map(|v| v.to_string_lossy().into_owned());
        }
        if let Some(val) = s.strip_prefix("--resume=") {
            return Some(val.to_owned());
        }
    }
    None
}

/// Resolve a PID for a specific session given all processes sharing the same cwd.
///
/// Matching priority (highest → lowest):
/// 1. Exact `--resume <session_id>` match → always precise.
/// 2. Parent-child filtering: drop any claude process whose parent is also a
///    claude process in this workspace (those are subagent child processes).
///    If exactly one "root" process remains → precise.
/// 3. Single process → precise regardless.
/// 4. Multiple unresolvable processes → imprecise (first as representative).
fn resolve_pid(procs: &[CliProcess], session_id: &str) -> (Option<u32>, bool) {
    if procs.is_empty() {
        return (None, false);
    }

    // Rule 1: exact --resume match.
    if let Some(p) = procs.iter().find(|p| {
        p.resume_session_id.as_deref() == Some(session_id)
    }) {
        return (Some(p.pid), true);
    }

    // Rule 2: filter out child claude processes (subagents).
    // A process is a "child" if its parent PID is also in this workspace's process set.
    let pid_set: std::collections::HashSet<u32> = procs.iter().map(|p| p.pid).collect();
    let roots: Vec<&CliProcess> = procs.iter().filter(|p| {
        !p.ppid.map_or(false, |ppid| pid_set.contains(&ppid))
    }).collect();

    match roots.len() {
        0 => (Some(procs[0].pid), false), // shouldn't happen; fall back
        1 => (Some(roots[0].pid), true),
        _ => (Some(roots[0].pid), false), // still ambiguous after filtering
    }
}

/// Scan all running `claude` processes.
/// Uses sysinfo for cross-platform support (macOS, Linux, Windows).
pub fn scan_cli_processes() -> Vec<CliProcess> {
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
    let matched_pids: Vec<_> = sys
        .processes()
        .iter()
        .filter(|(_, p)| {
            let name = p.name().to_string_lossy();
            name == "claude" || name == "claude.exe"
        })
        .map(|(pid, _)| *pid)
        .collect();

    // Phase 2: read cwd only for matched processes.
    if !matched_pids.is_empty() {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&matched_pids),
            true,
            ProcessRefreshKind::nothing()
                .with_cwd(UpdateKind::Always),
        );
    }

    for pid in &matched_pids {
        if let Some(process) = sys.process(*pid) {
            if let Some(cwd) = process.cwd() {
                if let Some(path) = cwd.to_str() {
                    let resume_session_id = extract_resume_id(process.cmd());
                    let ppid = process.parent().map(|p| p.as_u32());
                    result.push(CliProcess {
                        pid: pid.as_u32(),
                        ppid,
                        cwd: path.to_string(),
                        resume_session_id,
                    });
                }
            }
        }
    }
    result
}

// ── IDE session scanning ─────────────────────────────────────────────────────

pub fn scan_ide_sessions(claude_dir: &Path) -> Vec<IdeSession> {
    let ide_dir = claude_dir.join("ide");
    let mut sessions = Vec::new();

    let Ok(entries) = fs::read_dir(&ide_dir) else {
        return sessions;
    };

    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("lock") {
            continue;
        }
        let Ok(content) = fs::read_to_string(&path) else {
            continue;
        };
        let Ok(lock): Result<LockFile, _> = serde_json::from_str(&content) else {
            continue;
        };
        if is_process_alive(lock.pid) {
            sessions.push(IdeSession {
                pid: lock.pid,
                workspace_folders: lock.workspace_folders,
                ide_name: lock.ide_name,
            });
        }
    }
    sessions
}

// ── JSONL parsing ────────────────────────────────────────────────────────────

fn determine_status(
    last_lines: &[Value],
    file_age_secs: f64,
    hook_state: Option<&HookState>,
) -> SessionStatus {
    // Phase 0: Hook-based overrides for stale JSONL scenarios.
    // Hooks give us definitive signals that are more reliable than file-age guessing.
    // Only apply when the JSONL is not actively streaming (file_age >= 8s),
    // so we don't override fine-grained streaming detection.
    if file_age_secs >= 8.0 {
        match hook_state {
            Some(HookState::ToolExecuting) => return SessionStatus::Executing,
            Some(HookState::ModelProcessing) => return SessionStatus::Thinking,
            Some(HookState::Stopped) => return SessionStatus::WaitingInput,
            _ => {}
        }
    }

    if file_age_secs < 8.0 {
        // Find the current turn: everything after the last user message.
        let turn_start = last_lines
            .iter()
            .rposition(|v| v.get("type").and_then(|t| t.as_str()) == Some("user"))
            .map(|i| i + 1)
            .unwrap_or(0);

        // Look at the LAST incomplete (stop_reason=null) assistant message in the turn,
        // but only if no completed assistant message exists after it. Stale partials
        // left behind after a completed response must not override the final status.
        let last_partial_idx = last_lines[turn_start..].iter().rposition(|v| {
            if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
                return false;
            }
            let stop = v
                .get("message")
                .and_then(|m| m.get("stop_reason"));
            // stop_reason absent or null → still streaming
            stop.map_or(true, |s| s.is_null())
        });

        // Check whether a completed assistant message appears after the last partial.
        // If so, the partial is stale and should be ignored.
        let last_partial = last_partial_idx.and_then(|pidx| {
            let abs_pidx = turn_start + pidx;
            let has_completed_after = last_lines[abs_pidx + 1..].iter().any(|v| {
                if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
                    return false;
                }
                let stop = v
                    .get("message")
                    .and_then(|m| m.get("stop_reason"));
                // stop_reason present and non-null → completed
                stop.map_or(false, |s| !s.is_null())
            });
            if has_completed_after {
                None
            } else {
                Some(&last_lines[abs_pidx])
            }
        });

        if let Some(partial) = last_partial {
            let block_types: Vec<&str> = partial
                .get("message")
                .and_then(|m| m.get("content"))
                .and_then(|c| c.as_array())
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter_map(|b| b.get("type").and_then(|t| t.as_str()))
                        .collect()
                })
                .unwrap_or_default();

            if block_types.contains(&"thinking") {
                return SessionStatus::Thinking;
            }
            if block_types.contains(&"tool_use") {
                return SessionStatus::Executing;
            }
            return SessionStatus::Streaming;
        }

        // No incomplete message found — model may have just finished writing.
        // Fall through to check stop_reason of the last complete message.
    }

    // Check what the last meaningful line is to distinguish "tool executing" vs "model thinking".
    let last_meaningful = last_lines.iter().rev().find(|v| {
        matches!(
            v.get("type").and_then(|t| t.as_str()),
            Some("user") | Some("assistant")
        )
    });

    if let Some(last) = last_meaningful {
        let last_type = last.get("type").and_then(|t| t.as_str());

        if last_type == Some("user") {
            // Last write was a user message — the model is thinking about it.
            // This covers both: tool_result received (model thinking after tool execution)
            // and fresh user message (model doing initial/extended thinking before first write).
            if file_age_secs < 120.0 {
                return SessionStatus::Thinking;
            }
        }

        if last_type == Some("assistant") {
            let stop_value = last
                .get("message")
                .and_then(|m| m.get("stop_reason"));
            let stop_reason = stop_value.and_then(|s| s.as_str());
            let stop_is_null = stop_value.map_or(true, |s| s.is_null());

            if stop_is_null && file_age_secs < 120.0 {
                // Still streaming (stop_reason absent or null).
                // Check content blocks to determine what the model is outputting.
                let block_types: Vec<&str> = last
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_array())
                    .map(|blocks| {
                        blocks
                            .iter()
                            .filter_map(|b| b.get("type").and_then(|t| t.as_str()))
                            .collect()
                    })
                    .unwrap_or_default();

                if block_types.contains(&"tool_use") {
                    return SessionStatus::Executing;
                }
                if block_types.contains(&"thinking") {
                    return SessionStatus::Thinking;
                }
                return SessionStatus::Streaming;
            }

            match stop_reason {
                Some("end_turn") if file_age_secs < 300.0 => return SessionStatus::WaitingInput,
                // Last write was a tool_use — the tool is still executing.
                Some("tool_use") if file_age_secs < 60.0 => return SessionStatus::Executing,
                _ => {}
            }
        }
    }

    if file_age_secs < 30.0 {
        SessionStatus::Active
    } else {
        SessionStatus::Idle
    }
}

// ── Context window helpers ────────────────────────────────────────────────────

/// Best-effort lookup of a model's input-context-window size (in tokens).
/// Returns `None` when the model family is unrecognised so the caller can
/// decide whether to fall back to a default or skip the computation entirely.
pub fn context_window_for_model(model: &str) -> Option<u64> {
    let m = model.to_lowercase();

    // ── Anthropic / Claude ──────────────────────────────────────────────
    // All Claude 3 / 3.5 / 4.x models: 200 000 input tokens.
    if m.starts_with("claude-") {
        return Some(200_000);
    }

    // ── OpenAI ──────────────────────────────────────────────────────────
    // o3 / o4-mini: 200 000
    if m.starts_with("o3") || m.starts_with("o4") {
        return Some(200_000);
    }
    // GPT-4o / GPT-4o-mini: 128 000
    if m.starts_with("gpt-4o") {
        return Some(128_000);
    }
    // GPT-4.1: 1 048 576
    if m.starts_with("gpt-4.1") {
        return Some(1_048_576);
    }
    // GPT-4-turbo / GPT-4-1106+: 128 000
    if m.starts_with("gpt-4-turbo") || m.starts_with("gpt-4-1106") || m.starts_with("gpt-4-0125") {
        return Some(128_000);
    }
    // GPT-4 (base 8k)
    if m.starts_with("gpt-4") {
        return Some(8_192);
    }

    // ── Google ──────────────────────────────────────────────────────────
    if m.contains("gemini") {
        return Some(1_000_000);
    }

    None
}

/// Compute context-window utilisation (0.0 – 1.0) from raw token counts.
/// Returns `None` when the model is unrecognised.
pub fn compute_context_percent(input_tokens: u64, model: Option<&str>) -> Option<f64> {
    let window = context_window_for_model(model?)?;
    if window == 0 {
        return None;
    }
    Some((input_tokens as f64 / window as f64).min(1.0))
}

fn compute_token_stats(lines: &[&str]) -> (f64, u64) {
    let mut total_output: u64 = 0;
    let mut timed_tokens: Vec<(f64, u64)> = Vec::new();
    let mut seen_msg_ids: HashSet<String> = HashSet::new();

    for line in lines {
        let Ok(v): Result<Value, _> = serde_json::from_str(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
            continue;
        };
        // Only count finalized messages
        if msg.get("stop_reason").map_or(true, |s| s.is_null()) {
            continue;
        }
        let msg_id = msg
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or_default()
            .to_string();
        if !msg_id.is_empty() {
            if seen_msg_ids.contains(&msg_id) {
                continue;
            }
            seen_msg_ids.insert(msg_id);
        }

        let output_tokens = msg
            .get("usage")
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        total_output += output_tokens;

        // Timestamp for speed
        if let Some(ts_str) = v.get("timestamp").and_then(|t| t.as_str()) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                timed_tokens.push((dt.timestamp() as f64, output_tokens));
            }
        }
    }

    // Speed: tokens/s over the last 5-minute window
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

/// Extract context-window usage from the last finalized assistant message in
/// a Claude-Code JSONL session.  Returns `(input_tokens_used, model_name)`.
pub fn extract_last_context_usage(lines: &[&str]) -> Option<(u64, String)> {
    let mut last: Option<(u64, String)> = None;
    let mut seen_msg_ids: HashSet<String> = HashSet::new();

    for line in lines {
        let Ok(v): Result<Value, _> = serde_json::from_str(line) else {
            continue;
        };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else {
            continue;
        };
        if msg.get("stop_reason").map_or(true, |s| s.is_null()) {
            continue;
        }
        let msg_id = msg
            .get("id")
            .and_then(|i| i.as_str())
            .unwrap_or_default()
            .to_string();
        if !msg_id.is_empty() {
            if seen_msg_ids.contains(&msg_id) {
                continue;
            }
            seen_msg_ids.insert(msg_id);
        }

        let usage = msg.get("usage");
        let input = usage
            .and_then(|u| u.get("input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_create = usage
            .and_then(|u| u.get("cache_creation_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let cache_read = usage
            .and_then(|u| u.get("cache_read_input_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        let total_input = input + cache_create + cache_read;

        if total_input > 0 {
            let model = msg
                .get("model")
                .and_then(|m| m.as_str())
                .unwrap_or("")
                .to_string();
            last = Some((total_input, model));
        }
    }

    last
}

fn extract_model(last_lines: &[Value]) -> Option<String> {
    for msg in last_lines.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let model = msg
            .get("message")
            .and_then(|m| m.get("model"))
            .and_then(|m| m.as_str())
            .unwrap_or_default();
        if !model.is_empty() && model != "unknown" && model != "<synthetic>" {
            return Some(model.to_string());
        }
    }
    None
}

fn has_thinking_blocks(last_lines: &[Value]) -> bool {
    for msg in last_lines.iter() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        if let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        {
            for block in content {
                if block.get("type").and_then(|t| t.as_str()) == Some("thinking") {
                    return true;
                }
            }
        }
    }
    false
}

fn extract_last_text(last_lines: &[Value]) -> Option<String> {
    for msg in last_lines.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in content.iter().rev() {
            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                    let preview: String = text.chars().take(200).collect();
                    return Some(preview);
                }
            }
        }
    }
    None
}

fn extract_last_skill(last_lines: &[Value]) -> Option<String> {
    for msg in last_lines.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };
        for block in content.iter().rev() {
            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                && block.get("name").and_then(|n| n.as_str()) == Some("Skill")
            {
                if let Some(skill) = block
                    .get("input")
                    .and_then(|i| i.get("skill"))
                    .and_then(|s| s.as_str())
                {
                    return Some(skill.to_string());
                }
            }
        }
    }
    None
}

pub fn parse_session_info(
    jsonl_path: &Path,
    session_id: String,
    workspace_path: String,
    workspace_name: String,
    ide_name: Option<String>,
    is_subagent: bool,
    parent_session_id: Option<String>,
    agent_type: Option<String>,
    agent_description: Option<String>,
    meta_model: Option<String>,
    meta_thinking_level: Option<String>,
    pid: Option<u32>,
    pid_precise: bool,
    hook_state: Option<&HookState>,
) -> Option<SessionInfo> {
    let metadata = fs::metadata(jsonl_path).ok()?;
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

    // Skip sessions older than 7 days
    if age > Duration::from_secs(7 * 24 * 3600) {
        return None;
    }

    let content = fs::read_to_string(jsonl_path).ok()?;
    let all_lines: Vec<&str> = content.lines().collect();

    // Last 100 lines for status
    let start = all_lines.len().saturating_sub(100);
    let last_n: Vec<Value> = all_lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let status = determine_status(&last_n, age.as_secs_f64(), hook_state);
    let (token_speed, total_output_tokens) = compute_token_stats(&all_lines);
    let context_percent = extract_last_context_usage(&all_lines)
        .and_then(|(used, model)| compute_context_percent(used, Some(&model)));
    let last_message_preview = extract_last_text(&last_n);

    let slug = last_n
        .iter()
        .filter_map(|v| v.get("slug").and_then(|s| s.as_str()).map(|s| s.to_string()))
        .last();

    // ai-title appears near the start of the file; scan all lines
    let ai_title = all_lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("ai-title"))
        .and_then(|v| v.get("aiTitle").and_then(|t| t.as_str()).map(|s| s.to_string()));

    let model = meta_model.or_else(|| extract_model(&last_n));
    let last_skill = extract_last_skill(&last_n);

    // Prefer explicit thinking level from meta; fall back to detecting thinking blocks
    let thinking_level = meta_thinking_level.or_else(|| {
        if has_thinking_blocks(&last_n) {
            Some("thinking".to_string())
        } else {
            None
        }
    });

    Some(SessionInfo {
        id: session_id,
        workspace_path,
        workspace_name,
        ide_name,
        is_subagent,
        parent_session_id,
        agent_type,
        agent_description,
        slug,
        ai_title,
        status,
        token_speed,
        total_output_tokens,
        last_message_preview,
        last_activity_ms,
        created_at_ms,
        jsonl_path: jsonl_path.to_string_lossy().to_string(),
        model,
        thinking_level,
        pid,
        pid_precise,
        context_percent,
        last_skill,
        agent_source: "claude-code".to_string(),
        last_outcome: None,
    })
}

// ── Scan cache ───────────────────────────────────────────────────────────────

/// Caches expensive operations across rescans: process-table lookups and
/// already-parsed session files whose mtime hasn't changed.
pub struct ScanCache {
    /// Cached `scan_cli_processes()` result + timestamp.
    pub process_cache: Mutex<(Instant, Vec<CliProcess>)>,
    /// JSONL path → (mtime_ms, SessionInfo).
    pub session_cache: Mutex<HashMap<String, (u64, SessionInfo)>>,
}

impl ScanCache {
    pub fn new() -> Self {
        Self {
            process_cache: Mutex::new((Instant::now() - Duration::from_secs(999), Vec::new())),
            session_cache: Mutex::new(HashMap::new()),
        }
    }
}

/// Downgrade a cached session's status when the file hasn't been touched
/// and enough wall-clock time has elapsed.
fn age_out_status(info: &mut SessionInfo, age_secs: f64) {
    let idle = match info.status {
        SessionStatus::Streaming if age_secs >= 8.0 => true,
        SessionStatus::Thinking if age_secs >= 120.0 => true,
        SessionStatus::Executing if age_secs >= 60.0 => true,
        SessionStatus::Processing if age_secs >= 60.0 => true,
        SessionStatus::Active if age_secs >= 30.0 => true,
        SessionStatus::WaitingInput if age_secs >= 300.0 => true,
        SessionStatus::Delegating if age_secs >= 300.0 => true,
        _ => false,
    };
    if idle {
        info.status = SessionStatus::Idle;
        info.token_speed = 0.0;
    }
}

/// Check if a cached session entry is still valid (mtime matches).
/// Returns `(cached_info, age_secs)` on hit.
fn check_session_cache(
    path: &Path,
    cache: &HashMap<String, (u64, SessionInfo)>,
) -> Option<(SessionInfo, f64)> {
    let metadata = fs::metadata(path).ok()?;
    let last_modified = metadata.modified().ok()?;
    let mtime_ms = last_modified
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;
    let age_secs = SystemTime::now()
        .duration_since(last_modified)
        .unwrap_or(Duration::from_secs(3600))
        .as_secs_f64();

    if age_secs > 7.0 * 24.0 * 3600.0 {
        return None;
    }

    let key = path.to_string_lossy();
    let (cached_mt, cached_info) = cache.get(key.as_ref())?;
    if *cached_mt != mtime_ms {
        return None;
    }

    Some((cached_info.clone(), age_secs))
}

// ── Public entry point ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SubagentMeta {
    #[serde(rename = "agentType")]
    agent_type: Option<String>,
    description: Option<String>,
    model: Option<String>,
    #[serde(rename = "thinkingLevel")]
    thinking_level: Option<String>,
}

pub fn scan_claude_sessions(claude_dir: &Path, scan_cache: &ScanCache) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    let ide_sessions = scan_ide_sessions(claude_dir);

    // Reuse cached process list if fresh (< 10 s).
    let cli_processes = {
        let mut guard = scan_cache.process_cache.lock().unwrap();
        if guard.0.elapsed() > Duration::from_secs(10) {
            guard.1 = scan_cli_processes();
            guard.0 = Instant::now();
        }
        guard.1.clone()
    };

    let hook_states = crate::hooks::read_hook_states();
    let session_cache_snapshot = scan_cache.session_cache.lock().unwrap().clone();

    let projects_dir = claude_dir.join("projects");
    let Ok(workspace_entries) = fs::read_dir(&projects_dir) else {
        return sessions;
    };

    for workspace_entry in workspace_entries.flatten() {
        let workspace_dir = workspace_entry.path();
        if !workspace_dir.is_dir() {
            continue;
        }

        let encoded = workspace_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        // Find associated IDE session by encoding the lock file paths and comparing to the
        // directory name directly.  This avoids the lossy decode round-trip: a workspace named
        // "claw-fleet" encodes to "-Users-…-claw-fleet" but decodes to "/Users/…/claw/fleet".
        let ide = ide_sessions.iter().find(|ide| {
            ide.workspace_folders
                .iter()
                .any(|f| encode_workspace_path(f) == encoded)
        });

        // Use the exact path from the lock file when available; fall back to lossy decode.
        let workspace_path = ide
            .and_then(|s| {
                s.workspace_folders
                    .iter()
                    .find(|f| encode_workspace_path(f) == encoded)
            })
            .cloned()
            .unwrap_or_else(|| decode_workspace_path(&encoded));

        let ws_name = workspace_name(&workspace_path);
        let ide_name = ide.map(|s| s.ide_name.clone());

        // Collect all CLI processes for this workspace (may be >1 when subagents are running).
        // PID: use Claude CLI process only (not the IDE PID — killing the IDE PID would
        // terminate the editor itself, not just the Claude session).
        let procs_in_cwd: Vec<CliProcess> = cli_processes
            .iter()
            .filter(|p| p.cwd == workspace_path)
            .cloned()
            .collect();

        let Ok(entries) = fs::read_dir(&workspace_dir) else {
            continue;
        };

        for entry in entries.flatten() {
            let path = entry.path();

            // Main session JSONL files
            if path.is_file() && path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                let session_id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or_default()
                    .to_string();

                let (session_pid, pid_precise) = resolve_pid(&procs_in_cwd, &session_id);

                // Try session cache first (skip re-reading unchanged files).
                if let Some((mut info, age)) = check_session_cache(&path, &session_cache_snapshot) {
                    age_out_status(&mut info, age);
                    info.pid = session_pid;
                    info.pid_precise = pid_precise;
                    info.ide_name = ide_name.clone();
                    sessions.push(info);
                } else if let Some(info) = parse_session_info(
                    &path,
                    session_id.clone(),
                    workspace_path.clone(),
                    ws_name.clone(),
                    ide_name.clone(),
                    false,
                    None,
                    None,
                    None,
                    None,
                    None,
                    session_pid,
                    pid_precise,
                    hook_states.get(&session_id),
                ) {
                    scan_cache.session_cache.lock().unwrap()
                        .insert(path.to_string_lossy().to_string(), (info.last_activity_ms, info.clone()));
                    sessions.push(info);
                }
            }

            // Subagent directories: <session-uuid>/subagents/agent-*.jsonl
            if path.is_dir() {
                let parent_session_id = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default()
                    .to_string();

                let subagents_dir = path.join("subagents");
                let Ok(agent_entries) = fs::read_dir(&subagents_dir) else {
                    continue;
                };

                for agent_entry in agent_entries.flatten() {
                    let agent_path = agent_entry.path();
                    if agent_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }

                    let agent_id = agent_path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();

                    // Read optional meta.json
                    let meta_path = agent_path.with_extension("meta.json");
                    let meta = fs::read_to_string(&meta_path)
                        .ok()
                        .and_then(|s| serde_json::from_str::<SubagentMeta>(&s).ok());

                    let agent_type = meta.as_ref().and_then(|m| m.agent_type.clone());
                    let agent_description = meta.as_ref().and_then(|m| m.description.clone());
                    let meta_model = meta.as_ref().and_then(|m| m.model.clone());
                    let meta_thinking_level = meta.and_then(|m| m.thinking_level.clone());

                    // Subagents share the parent's PID resolution; never precise on their own
                    // since we can't kill just the subagent independently.
                    let (sub_pid, _) = resolve_pid(&procs_in_cwd, &parent_session_id);

                    // Try session cache first for subagents too.
                    if let Some((mut info, age)) = check_session_cache(&agent_path, &session_cache_snapshot) {
                        age_out_status(&mut info, age);
                        info.pid = sub_pid;
                        info.pid_precise = false;
                        info.ide_name = ide_name.clone();
                        sessions.push(info);
                    } else if let Some(info) = parse_session_info(
                        &agent_path,
                        agent_id.clone(),
                        workspace_path.clone(),
                        ws_name.clone(),
                        ide_name.clone(),
                        true,
                        Some(parent_session_id.clone()),
                        agent_type,
                        agent_description,
                        meta_model,
                        meta_thinking_level,
                        sub_pid,
                        false, // subagents are never pid_precise: stop parent instead
                        hook_states.get(&agent_id),
                    ) {
                        scan_cache.session_cache.lock().unwrap()
                            .insert(agent_path.to_string_lossy().to_string(), (info.last_activity_ms, info.clone()));
                        sessions.push(info);
                    }
                }
            }
        }
    }

    // Promote main sessions to Delegating if they have at least one actively-working subagent.
    // A subagent that is WaitingInput has finished its turn and should not cause the parent
    // to show as Delegating — otherwise the parent's own WaitingInput status gets hidden.
    let active_parent_ids: std::collections::HashSet<String> = sessions
        .iter()
        .filter(|s| {
            s.is_subagent
                && matches!(
                    s.status,
                    SessionStatus::Thinking
                        | SessionStatus::Executing
                        | SessionStatus::Streaming
                        | SessionStatus::Delegating
                        | SessionStatus::Processing
                )
        })
        .filter_map(|s| s.parent_session_id.clone())
        .collect();

    for session in &mut sessions {
        if !session.is_subagent
            && session.parent_session_id.is_none()
            && active_parent_ids.contains(&session.id)
            && matches!(
                session.status,
                SessionStatus::Active | SessionStatus::Idle | SessionStatus::WaitingInput | SessionStatus::Processing
            )
        {
            session.status = SessionStatus::Delegating;
        }
    }

    // Prune stale entries from session cache.
    {
        let live_paths: HashSet<String> = sessions.iter().map(|s| s.jsonl_path.clone()).collect();
        scan_cache.session_cache.lock().unwrap().retain(|k, _| live_paths.contains(k));
    }

    // Sort: active first, then by created_at_ms asc (oldest first = stable order)
    sessions.sort_by(|a, b| {
        let a_active = matches!(
            a.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
        );
        let b_active = matches!(
            b.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
        );
        b_active
            .cmp(&a_active)
            .then(a.created_at_ms.cmp(&b.created_at_ms))
    });

    sessions
}

/// Sort sessions: active first, then by created_at_ms asc.
pub fn sort_sessions(sessions: &mut Vec<SessionInfo>) {
    sessions.sort_by(|a, b| {
        let a_active = matches!(
            a.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
        );
        let b_active = matches!(
            b.status,
            SessionStatus::Thinking
                | SessionStatus::Executing
                | SessionStatus::Streaming
                | SessionStatus::Delegating
                | SessionStatus::Processing
                | SessionStatus::WaitingInput
        );
        b_active
            .cmp(&a_active)
            .then(a.created_at_ms.cmp(&b.created_at_ms))
    });
}

/// Scan all agent sources and merge into a single sorted list.
pub fn scan_sessions(claude_dir: &Path, scan_cache: &ScanCache) -> Vec<SessionInfo> {
    let mut sessions = scan_claude_sessions(claude_dir, scan_cache);
    sort_sessions(&mut sessions);
    sessions
}

/// Scan all registered agent sources and merge into a single sorted list.
pub fn scan_all_sources(sources: &[Box<dyn crate::agent_source::AgentSource>]) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    for source in sources {
        if source.is_available() {
            sessions.extend(source.scan_sessions());
        }
    }
    sort_sessions(&mut sessions);
    sessions
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    // ── Helper builders ─────────────────────────────────────────────────────

    /// Build an assistant message with given content blocks and stop_reason.
    fn assistant_msg(blocks: Vec<Value>, stop_reason: Option<&str>) -> Value {
        json!({
            "type": "assistant",
            "message": {
                "role": "assistant",
                "content": blocks,
                "stop_reason": stop_reason,
                "model": "claude-sonnet-4-20250514",
                "usage": { "output_tokens": 100 }
            }
        })
    }

    fn assistant_msg_with_id(blocks: Vec<Value>, stop_reason: Option<&str>, id: &str, ts: &str) -> String {
        json!({
            "type": "assistant",
            "timestamp": ts,
            "message": {
                "id": id,
                "role": "assistant",
                "content": blocks,
                "stop_reason": stop_reason,
                "model": "claude-sonnet-4-20250514",
                "usage": { "output_tokens": 50 }
            }
        }).to_string()
    }

    fn user_msg() -> Value {
        json!({ "type": "user", "message": { "role": "user", "content": [{"type": "text", "text": "hello"}] } })
    }

    fn text_block(text: &str) -> Value {
        json!({"type": "text", "text": text})
    }

    fn thinking_block() -> Value {
        json!({"type": "thinking", "thinking": "hmm..."})
    }

    fn tool_use_block(name: &str) -> Value {
        json!({"type": "tool_use", "name": name, "input": {}})
    }

    fn skill_block(skill: &str) -> Value {
        json!({"type": "tool_use", "name": "Skill", "input": {"skill": skill}})
    }

    fn make_session(status: SessionStatus) -> SessionInfo {
        SessionInfo {
            id: "test-session".into(),
            workspace_path: "/tmp/test".into(),
            workspace_name: "test".into(),
            ide_name: None,
            is_subagent: false,
            parent_session_id: None,
            agent_type: None,
            agent_description: None,
            slug: None,
            ai_title: None,
            status,
            token_speed: 10.0,
            total_output_tokens: 500,
            last_message_preview: None,
            last_activity_ms: 0,
            created_at_ms: 0,
            jsonl_path: "/tmp/test.jsonl".into(),
            model: None,
            thinking_level: None,
            pid: None,
            pid_precise: false,
            last_skill: None,
            context_percent: None,
            agent_source: "claude-code".into(),
            last_outcome: None,
        }
    }

    // ── determine_status tests ──────────────────────────────────────────────

    #[test]
    fn status_streaming_thinking_blocks() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![thinking_block()], None), // stop_reason=null → streaming
        ];
        assert_eq!(determine_status(&lines, 2.0, None), SessionStatus::Thinking);
    }

    #[test]
    fn status_streaming_tool_use_blocks() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![text_block("let me check"), tool_use_block("Read")], None),
        ];
        assert_eq!(determine_status(&lines, 1.0, None), SessionStatus::Executing);
    }

    #[test]
    fn status_streaming_text_only() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![text_block("Hello world")], None),
        ];
        assert_eq!(determine_status(&lines, 3.0, None), SessionStatus::Streaming);
    }

    #[test]
    fn status_end_turn_waiting_input() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![text_block("Done!")], Some("end_turn")),
        ];
        assert_eq!(determine_status(&lines, 10.0, None), SessionStatus::WaitingInput);
    }

    #[test]
    fn status_end_turn_too_old_becomes_idle() {
        let lines = vec![
            assistant_msg(vec![text_block("Done!")], Some("end_turn")),
        ];
        assert_eq!(determine_status(&lines, 500.0, None), SessionStatus::Idle);
    }

    #[test]
    fn status_tool_use_stop_reason_executing() {
        let lines = vec![
            assistant_msg(vec![tool_use_block("Bash")], Some("tool_use")),
        ];
        assert_eq!(determine_status(&lines, 15.0, None), SessionStatus::Executing);
    }

    #[test]
    fn status_tool_use_too_old_becomes_idle() {
        let lines = vec![
            assistant_msg(vec![tool_use_block("Bash")], Some("tool_use")),
        ];
        assert_eq!(determine_status(&lines, 120.0, None), SessionStatus::Idle);
    }

    #[test]
    fn status_user_message_last_thinking() {
        let lines = vec![user_msg()];
        assert_eq!(determine_status(&lines, 5.0, None), SessionStatus::Thinking);
    }

    #[test]
    fn status_user_message_too_old() {
        let lines = vec![user_msg()];
        assert_eq!(determine_status(&lines, 200.0, None), SessionStatus::Idle);
    }

    #[test]
    fn status_no_meaningful_lines_recent() {
        let lines: Vec<Value> = vec![];
        assert_eq!(determine_status(&lines, 10.0, None), SessionStatus::Active);
    }

    #[test]
    fn status_no_meaningful_lines_old() {
        let lines: Vec<Value> = vec![];
        assert_eq!(determine_status(&lines, 60.0, None), SessionStatus::Idle);
    }

    #[test]
    fn status_hook_tool_executing_overrides() {
        let lines = vec![
            assistant_msg(vec![text_block("old text")], Some("end_turn")),
        ];
        assert_eq!(
            determine_status(&lines, 20.0, Some(&HookState::ToolExecuting)),
            SessionStatus::Executing,
        );
    }

    #[test]
    fn status_hook_model_processing_overrides() {
        let lines = vec![
            assistant_msg(vec![text_block("old")], Some("end_turn")),
        ];
        assert_eq!(
            determine_status(&lines, 20.0, Some(&HookState::ModelProcessing)),
            SessionStatus::Thinking,
        );
    }

    #[test]
    fn status_hook_stopped_overrides() {
        let lines = vec![user_msg()];
        assert_eq!(
            determine_status(&lines, 20.0, Some(&HookState::Stopped)),
            SessionStatus::WaitingInput,
        );
    }

    #[test]
    fn status_hook_ignored_when_streaming() {
        let lines = vec![
            user_msg(),
            assistant_msg(vec![thinking_block()], None),
        ];
        assert_eq!(
            determine_status(&lines, 2.0, Some(&HookState::Stopped)),
            SessionStatus::Thinking,
        );
    }

    // ── compute_token_stats tests ───────────────────────────────────────────

    #[test]
    fn token_stats_empty_lines() {
        let lines: Vec<&str> = vec![];
        let (speed, total) = compute_token_stats(&lines);
        assert_eq!(total, 0);
        assert_eq!(speed, 0.0);
    }

    #[test]
    fn token_stats_non_assistant_ignored() {
        let line = json!({"type": "user", "message": {"content": []}}).to_string();
        let lines: Vec<&str> = vec![&line];
        let (_, total) = compute_token_stats(&lines);
        assert_eq!(total, 0);
    }

    #[test]
    fn token_stats_null_stop_reason_ignored() {
        let line = json!({
            "type": "assistant",
            "message": {
                "stop_reason": null,
                "usage": {"output_tokens": 100}
            }
        }).to_string();
        let lines: Vec<&str> = vec![&line];
        let (_, total) = compute_token_stats(&lines);
        assert_eq!(total, 0);
    }

    #[test]
    fn token_stats_counts_finalized_tokens() {
        let line = json!({
            "type": "assistant",
            "message": {
                "id": "msg_1",
                "stop_reason": "end_turn",
                "usage": {"output_tokens": 200}
            }
        }).to_string();
        let lines: Vec<&str> = vec![&line];
        let (_, total) = compute_token_stats(&lines);
        assert_eq!(total, 200);
    }

    #[test]
    fn token_stats_deduplicates_by_id() {
        let line = json!({
            "type": "assistant",
            "message": {
                "id": "msg_dup",
                "stop_reason": "end_turn",
                "usage": {"output_tokens": 100}
            }
        }).to_string();
        let lines: Vec<&str> = vec![&line, &line];
        let (_, total) = compute_token_stats(&lines);
        assert_eq!(total, 100);
    }

    #[test]
    fn token_stats_speed_from_recent_timestamps() {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let ts1 = chrono::DateTime::from_timestamp(now as i64 - 60, 0).unwrap().to_rfc3339();
        let ts2 = chrono::DateTime::from_timestamp(now as i64 - 30, 0).unwrap().to_rfc3339();

        let l1 = assistant_msg_with_id(vec![], Some("end_turn"), "m1", &ts1);
        let l2 = assistant_msg_with_id(vec![], Some("end_turn"), "m2", &ts2);
        let lines: Vec<&str> = vec![&l1, &l2];
        let (speed, total) = compute_token_stats(&lines);
        assert_eq!(total, 100); // 50 + 50
        assert!(speed > 3.0 && speed < 4.0, "speed={speed}");
    }

    // ── extract_model tests ─────────────────────────────────────────────────

    #[test]
    fn extract_model_from_assistant() {
        let lines = vec![
            assistant_msg(vec![text_block("hi")], Some("end_turn")),
        ];
        assert_eq!(extract_model(&lines), Some("claude-sonnet-4-20250514".into()));
    }

    #[test]
    fn extract_model_ignores_unknown() {
        let lines = vec![json!({
            "type": "assistant",
            "message": {"model": "unknown", "content": [], "stop_reason": "end_turn"}
        })];
        assert_eq!(extract_model(&lines), None);
    }

    #[test]
    fn extract_model_ignores_synthetic() {
        let lines = vec![json!({
            "type": "assistant",
            "message": {"model": "<synthetic>", "content": [], "stop_reason": "end_turn"}
        })];
        assert_eq!(extract_model(&lines), None);
    }

    #[test]
    fn extract_model_ignores_user_messages() {
        let lines = vec![user_msg()];
        assert_eq!(extract_model(&lines), None);
    }

    // ── has_thinking_blocks tests ───────────────────────────────────────────

    #[test]
    fn thinking_blocks_present() {
        let lines = vec![
            assistant_msg(vec![thinking_block(), text_block("result")], Some("end_turn")),
        ];
        assert!(has_thinking_blocks(&lines));
    }

    #[test]
    fn thinking_blocks_absent() {
        let lines = vec![
            assistant_msg(vec![text_block("no thinking")], Some("end_turn")),
        ];
        assert!(!has_thinking_blocks(&lines));
    }

    // ── extract_last_text tests ─────────────────────────────────────────────

    #[test]
    fn extract_text_from_last_assistant() {
        let lines = vec![
            assistant_msg(vec![text_block("first message")], Some("end_turn")),
            assistant_msg(vec![text_block("second message")], Some("end_turn")),
        ];
        assert_eq!(extract_last_text(&lines), Some("second message".into()));
    }

    #[test]
    fn extract_text_truncates_to_200_chars() {
        let long_text = "a".repeat(300);
        let lines = vec![
            assistant_msg(vec![text_block(&long_text)], Some("end_turn")),
        ];
        let result = extract_last_text(&lines).unwrap();
        assert_eq!(result.len(), 200);
    }

    #[test]
    fn extract_text_returns_none_for_no_text() {
        let lines = vec![
            assistant_msg(vec![tool_use_block("Bash")], Some("tool_use")),
        ];
        assert_eq!(extract_last_text(&lines), None);
    }

    // ── extract_last_skill tests ────────────────────────────────────────────

    #[test]
    fn extract_skill_found() {
        let lines = vec![
            assistant_msg(vec![skill_block("commit")], Some("tool_use")),
        ];
        assert_eq!(extract_last_skill(&lines), Some("commit".into()));
    }

    #[test]
    fn extract_skill_not_found() {
        let lines = vec![
            assistant_msg(vec![tool_use_block("Read")], Some("tool_use")),
        ];
        assert_eq!(extract_last_skill(&lines), None);
    }

    // ── extract_resume_id tests ─────────────────────────────────────────────

    #[test]
    fn resume_id_long_flag() {
        let cmd: Vec<std::ffi::OsString> = vec!["claude".into(), "--resume".into(), "abc123".into()];
        assert_eq!(extract_resume_id(&cmd), Some("abc123".into()));
    }

    #[test]
    fn resume_id_short_flag() {
        let cmd: Vec<std::ffi::OsString> = vec!["claude".into(), "-r".into(), "xyz".into()];
        assert_eq!(extract_resume_id(&cmd), Some("xyz".into()));
    }

    #[test]
    fn resume_id_equals_syntax() {
        let cmd: Vec<std::ffi::OsString> = vec!["claude".into(), "--resume=sess42".into()];
        assert_eq!(extract_resume_id(&cmd), Some("sess42".into()));
    }

    #[test]
    fn resume_id_absent() {
        let cmd: Vec<std::ffi::OsString> = vec!["claude".into(), "--verbose".into()];
        assert_eq!(extract_resume_id(&cmd), None);
    }

    // ── resolve_pid tests ───────────────────────────────────────────────────

    #[test]
    fn resolve_pid_empty() {
        assert_eq!(resolve_pid(&[], "sess1"), (None, false));
    }

    #[test]
    fn resolve_pid_exact_resume_match() {
        let procs = vec![
            CliProcess { pid: 100, ppid: None, cwd: "/tmp".into(), resume_session_id: Some("sess1".into()) },
            CliProcess { pid: 200, ppid: None, cwd: "/tmp".into(), resume_session_id: None },
        ];
        assert_eq!(resolve_pid(&procs, "sess1"), (Some(100), true));
    }

    #[test]
    fn resolve_pid_single_process() {
        let procs = vec![
            CliProcess { pid: 42, ppid: None, cwd: "/tmp".into(), resume_session_id: None },
        ];
        assert_eq!(resolve_pid(&procs, "other"), (Some(42), true));
    }

    #[test]
    fn resolve_pid_parent_child_filtering() {
        let procs = vec![
            CliProcess { pid: 100, ppid: Some(1), cwd: "/tmp".into(), resume_session_id: None },
            CliProcess { pid: 200, ppid: Some(100), cwd: "/tmp".into(), resume_session_id: None },
        ];
        assert_eq!(resolve_pid(&procs, "any"), (Some(100), true));
    }

    #[test]
    fn resolve_pid_multiple_roots_imprecise() {
        let procs = vec![
            CliProcess { pid: 100, ppid: Some(1), cwd: "/tmp".into(), resume_session_id: None },
            CliProcess { pid: 200, ppid: Some(2), cwd: "/tmp".into(), resume_session_id: None },
        ];
        let (pid, precise) = resolve_pid(&procs, "any");
        assert!(pid.is_some());
        assert!(!precise);
    }

    // ── workspace_name / encode / decode tests ──────────────────────────────

    #[test]
    fn workspace_name_basic() {
        assert_eq!(workspace_name("/Users/foo/my-project"), "my-project");
    }

    #[test]
    fn workspace_name_trailing_slash() {
        assert_eq!(workspace_name("/Users/foo/bar/"), "bar");
    }

    #[test]
    fn workspace_name_root() {
        assert_eq!(workspace_name("/"), "/");
    }

    #[test]
    fn encode_decode_workspace_path() {
        let original = "/Users/foo/bar";
        let encoded = encode_workspace_path(original);
        assert_eq!(encoded, "-Users-foo-bar");
        let decoded = decode_workspace_path(&encoded);
        assert_eq!(decoded, original);
    }

    // ── age_out_status tests ────────────────────────────────────────────────

    #[test]
    fn age_out_streaming() {
        let mut s = make_session(SessionStatus::Streaming);
        age_out_status(&mut s, 7.0);
        assert_eq!(s.status, SessionStatus::Streaming);
        age_out_status(&mut s, 8.0);
        assert_eq!(s.status, SessionStatus::Idle);
        assert_eq!(s.token_speed, 0.0);
    }

    #[test]
    fn age_out_thinking() {
        let mut s = make_session(SessionStatus::Thinking);
        age_out_status(&mut s, 119.0);
        assert_eq!(s.status, SessionStatus::Thinking);
        age_out_status(&mut s, 120.0);
        assert_eq!(s.status, SessionStatus::Idle);
    }

    #[test]
    fn age_out_executing() {
        let mut s = make_session(SessionStatus::Executing);
        age_out_status(&mut s, 59.0);
        assert_eq!(s.status, SessionStatus::Executing);
        age_out_status(&mut s, 60.0);
        assert_eq!(s.status, SessionStatus::Idle);
    }

    #[test]
    fn age_out_waiting_input() {
        let mut s = make_session(SessionStatus::WaitingInput);
        age_out_status(&mut s, 299.0);
        assert_eq!(s.status, SessionStatus::WaitingInput);
        age_out_status(&mut s, 300.0);
        assert_eq!(s.status, SessionStatus::Idle);
    }

    #[test]
    fn age_out_idle_stays_idle() {
        let mut s = make_session(SessionStatus::Idle);
        s.token_speed = 0.0;
        age_out_status(&mut s, 9999.0);
        assert_eq!(s.status, SessionStatus::Idle);
    }
}
