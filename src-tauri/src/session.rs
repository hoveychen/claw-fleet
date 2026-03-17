use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
    pub workspace_folders: Vec<String>,
    pub ide_name: String,
}

// ── Exported types ───────────────────────────────────────────────────────────

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum SessionStatus {
    Streaming,    // file written < 2s ago
    Processing,   // last stop_reason = tool_use, recent activity
    WaitingInput, // last stop_reason = end_turn
    Active,       // file written < 30s ago
    Idle,         // no recent activity
}

#[derive(Serialize, Clone, Debug)]
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
    pub status: SessionStatus,
    pub token_speed: f64,
    pub total_output_tokens: u64,
    pub last_message_preview: Option<String>,
    pub last_activity_ms: u64,
    pub jsonl_path: String,
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
    // Best-effort; dashes inside path components are lost.
    let stripped = encoded.trim_start_matches('-');
    format!("/{}", stripped.replace('-', "/"))
}

fn workspace_name(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(path)
        .to_string()
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
                workspace_folders: lock.workspace_folders,
                ide_name: lock.ide_name,
            });
        }
    }
    sessions
}

// ── JSONL parsing ────────────────────────────────────────────────────────────

fn determine_status(last_lines: &[Value], file_age_secs: f64) -> SessionStatus {
    if file_age_secs < 2.0 {
        return SessionStatus::Streaming;
    }

    let last_assistant = last_lines.iter().rev().find(|v| {
        v.get("type").and_then(|t| t.as_str()) == Some("assistant")
    });

    if let Some(msg) = last_assistant {
        let stop_reason = msg
            .get("message")
            .and_then(|m| m.get("stop_reason"))
            .and_then(|s| s.as_str());

        match stop_reason {
            Some("end_turn") if file_age_secs < 300.0 => return SessionStatus::WaitingInput,
            Some("tool_use") if file_age_secs < 60.0 => return SessionStatus::Processing,
            _ => {}
        }
    }

    if file_age_secs < 30.0 {
        SessionStatus::Active
    } else {
        SessionStatus::Idle
    }
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

fn parse_session_info(
    jsonl_path: &Path,
    session_id: String,
    workspace_path: String,
    workspace_name: String,
    ide_name: Option<String>,
    is_subagent: bool,
    parent_session_id: Option<String>,
    agent_type: Option<String>,
    agent_description: Option<String>,
) -> Option<SessionInfo> {
    let metadata = fs::metadata(jsonl_path).ok()?;
    let last_modified = metadata.modified().ok()?;
    let last_activity_ms = last_modified
        .duration_since(UNIX_EPOCH)
        .ok()?
        .as_millis() as u64;

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

    let status = determine_status(&last_n, age.as_secs_f64());
    let (token_speed, total_output_tokens) = compute_token_stats(&all_lines);
    let last_message_preview = extract_last_text(&last_n);

    let slug = last_n
        .iter()
        .filter_map(|v| v.get("slug").and_then(|s| s.as_str()).map(|s| s.to_string()))
        .last();

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
        status,
        token_speed,
        total_output_tokens,
        last_message_preview,
        last_activity_ms,
        jsonl_path: jsonl_path.to_string_lossy().to_string(),
    })
}

// ── Public entry point ───────────────────────────────────────────────────────

#[derive(Deserialize)]
struct SubagentMeta {
    #[serde(rename = "agentType")]
    agent_type: Option<String>,
    description: Option<String>,
}

pub fn scan_sessions(claude_dir: &Path) -> Vec<SessionInfo> {
    let mut sessions = Vec::new();
    let ide_sessions = scan_ide_sessions(claude_dir);

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

        let workspace_path = decode_workspace_path(&encoded);
        let ws_name = workspace_name(&workspace_path);

        // Find associated IDE session (by workspace folder match)
        let ide = ide_sessions.iter().find(|ide| {
            ide.workspace_folders
                .iter()
                .any(|f| f == &workspace_path)
        });
        let ide_name = ide.map(|s| s.ide_name.clone());

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

                if let Some(info) = parse_session_info(
                    &path,
                    session_id,
                    workspace_path.clone(),
                    ws_name.clone(),
                    ide_name.clone(),
                    false,
                    None,
                    None,
                    None,
                ) {
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
                    let agent_description = meta.and_then(|m| m.description);

                    if let Some(info) = parse_session_info(
                        &agent_path,
                        agent_id,
                        workspace_path.clone(),
                        ws_name.clone(),
                        ide_name.clone(),
                        true,
                        Some(parent_session_id.clone()),
                        agent_type,
                        agent_description,
                    ) {
                        sessions.push(info);
                    }
                }
            }
        }
    }

    // Sort: active first, then by last_activity_ms desc
    sessions.sort_by(|a, b| {
        let a_active = matches!(
            a.status,
            SessionStatus::Streaming | SessionStatus::Processing | SessionStatus::WaitingInput
        );
        let b_active = matches!(
            b.status,
            SessionStatus::Streaming | SessionStatus::Processing | SessionStatus::WaitingInput
        );
        b_active
            .cmp(&a_active)
            .then(b.last_activity_ms.cmp(&a.last_activity_ms))
    });

    sessions
}
