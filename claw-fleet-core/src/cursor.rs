//! Cursor session scanning — reads composerData from Cursor's state.vscdb
//! (SQLite) and maps sessions into the shared `SessionInfo` model.
//!
//! Cursor stores agent data in `~/Library/Application Support/Cursor/User/globalStorage/state.vscdb`
//! in a `cursorDiskKV` table with keys like `composerData:<uuid>`.
//! Message data is stored as `bubbleId:<composerId>:<bubbleId>`.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::session::{SessionInfo, SessionStatus, compute_context_percent};

/// URI prefix for Cursor session identifiers (used in jsonl_path field).
pub const CURSOR_URI_PREFIX: &str = "cursor://";

// ── Helpers ──────────────────────────────────────────────────────────────────

pub fn get_cursor_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".cursor"))
}

/// Path to Cursor's main SQLite KV store.
fn state_vscdb_path() -> Option<PathBuf> {
    #[cfg(target_os = "macos")]
    {
        crate::session::real_home_dir().map(|h| {
            h.join("Library")
                .join("Application Support")
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb")
        })
    }
    #[cfg(target_os = "windows")]
    {
        std::env::var("APPDATA").ok().map(|appdata| {
            PathBuf::from(appdata)
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb")
        })
    }
    #[cfg(target_os = "linux")]
    {
        crate::session::real_home_dir().map(|h| {
            h.join(".config")
                .join("Cursor")
                .join("User")
                .join("globalStorage")
                .join("state.vscdb")
        })
    }
}

fn open_cursor_db() -> Result<rusqlite::Connection, String> {
    let db_path = state_vscdb_path().ok_or("Cursor DB path not found")?;
    if !db_path.exists() {
        return Err("state.vscdb not found".to_string());
    }
    let conn = rusqlite::Connection::open_with_flags(
        &db_path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_FULL_MUTEX,
    )
    .map_err(|e| format!("Cannot open state.vscdb: {e}"))?;

    // Avoid blocking on Cursor's WAL lock — return SQLITE_BUSY immediately.
    conn.busy_timeout(Duration::from_millis(500))
        .map_err(|e| format!("Cannot set busy timeout: {e}"))?;

    Ok(conn)
}

fn map_cursor_status(status: &str, last_updated_ms: u64) -> SessionStatus {
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let age = Duration::from_millis(now_ms.saturating_sub(last_updated_ms));

    // Cursor uses "aborted" as the in-flight status while a session is actively
    // running.  It flips to "completed" once the model finishes a turn.
    match status {
        "generating" | "streaming" => {
            if age < Duration::from_secs(8) {
                SessionStatus::Streaming
            } else if age < Duration::from_secs(30) {
                SessionStatus::Active
            } else {
                SessionStatus::Idle
            }
        }
        "aborted" => {
            // Cursor agent turns can run for 10+ minutes while lastUpdatedAt
            // is only set once near the start, so use a generous window.
            if age < Duration::from_secs(15) {
                SessionStatus::Streaming
            } else if age < Duration::from_secs(10 * 60) {
                SessionStatus::Active
            } else {
                SessionStatus::Idle
            }
        }
        "pending" | "queued" => SessionStatus::Processing,
        "completed" | "finished" | "done" => {
            // Cursor writes "completed" the instant a turn finishes, but the
            // user is likely still reading/reviewing. Use a wider window.
            if age < Duration::from_secs(15) {
                SessionStatus::Active
            } else if age < Duration::from_secs(3 * 60) {
                SessionStatus::WaitingInput
            } else {
                SessionStatus::Idle
            }
        }
        "error" | "cancelled" => SessionStatus::Idle,
        _ => {
            if age < Duration::from_secs(30) {
                SessionStatus::Active
            } else {
                SessionStatus::Idle
            }
        }
    }
}

fn extract_model_name(v: &Value) -> Option<String> {
    v.get("modelConfig")
        .and_then(|mc| mc.get("modelName"))
        .and_then(|m| m.as_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
}

fn workspace_name(path: &str) -> String {
    path.split('/')
        .filter(|s| !s.is_empty())
        .last()
        .unwrap_or(path)
        .to_string()
}

fn decode_workspace_path(encoded: &str) -> String {
    let stripped = encoded.trim_start_matches('-');
    let parts: Vec<&str> = stripped.split('-').collect();
    if parts.is_empty() {
        return "/".to_string();
    }
    crate::session::decode_workspace_path_with_parts(&parts)
}

// ── Token estimation ─────────────────────────────────────────────────────────

/// Approximate token count from character length.
/// English text averages ~4 chars/token; code is similar. This is intentionally
/// a rough estimate — Cursor doesn't expose real token counts for agentic sessions.
fn estimate_tokens_from_text(text: &str) -> u64 {
    let len = text.len() as u64;
    // ~4 chars per token, with a minimum of 1 token for non-empty text
    if len == 0 { 0 } else { (len / 4).max(1) }
}

/// Estimate token speed and total output tokens for each composer by reading
/// assistant bubble data from state.vscdb.
///
/// Returns a map of composer_id → (token_speed, total_output_tokens).
/// Token speed is computed over the last 5-minute window, matching Claude Code's approach.
/// Returns per-composer (speed, total_output, last_input_tokens).
fn estimate_cursor_token_stats(composer_ids: &[&str]) -> HashMap<String, (f64, u64, Option<u64>)> {
    let mut result: HashMap<String, (f64, u64, Option<u64>)> = HashMap::new();
    if composer_ids.is_empty() {
        return result;
    }

    let conn = match open_cursor_db() {
        Ok(c) => c,
        Err(_) => return result,
    };

    // Build a set for O(1) lookup of composer IDs we care about
    let cid_set: HashSet<&str> = composer_ids.iter().copied().collect();

    // Single range scan: fetch all bubbleId entries at once instead of N queries.
    // Uses range bounds for efficient index usage (avoids LIKE pattern scanning).
    let mut stmt = match conn.prepare(
        "SELECT key, value FROM cursorDiskKV WHERE key >= 'bubbleId:' AND key < 'bubbleId;'"
    ) {
        Ok(s) => s,
        Err(_) => return result,
    };

    // Per-composer accumulators: (total_output, timed_tokens, last_input_tokens)
    let mut accums: HashMap<String, (u64, Vec<(f64, u64)>, u64)> = HashMap::new();

    let rows = match stmt.query_map([], |row| {
        let key: String = row.get(0)?;
        let value: String = row.get(1)?;
        Ok((key, value))
    }) {
        Ok(r) => r,
        Err(_) => return result,
    };

    for row in rows.flatten() {
        let (key, raw) = row;

        // Key format: "bubbleId:<composerId>:<bubbleId>"
        let rest = match key.strip_prefix("bubbleId:") {
            Some(r) => r,
            None => continue,
        };
        let cid_end = match rest.find(':') {
            Some(i) => i,
            None => continue,
        };
        let cid = &rest[..cid_end];

        // Skip bubbles for composers we don't care about
        if !cid_set.contains(cid) {
            continue;
        }

        let Ok(b): Result<Value, _> = serde_json::from_str(&raw) else { continue };

        // Only count assistant bubbles (type=2)
        if b.get("type").and_then(|t| t.as_u64()) != Some(2) {
            continue;
        }

        // Try real tokenCount first; fall back to text-length estimation
        let real_output = b.get("tokenCount")
            .and_then(|tc| tc.get("outputTokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        let estimated = if real_output > 0 {
            real_output
        } else {
            let text_tokens = b.get("text")
                .and_then(|t| t.as_str())
                .map(estimate_tokens_from_text)
                .unwrap_or(0);
            let thinking_tokens: u64 = b.get("allThinkingBlocks")
                .and_then(|a| a.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|block| {
                            block.get("rawThinking")
                                .and_then(|t| t.as_str())
                                .map(estimate_tokens_from_text)
                        })
                        .sum()
                })
                .unwrap_or(0);
            text_tokens + thinking_tokens
        };

        let accum = accums.entry(cid.to_string()).or_insert_with(|| (0, Vec::new(), 0));
        accum.0 += estimated;

        // Track last assistant bubble's input tokens for context_percent
        let real_input = b.get("tokenCount")
            .and_then(|tc| tc.get("inputTokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        if real_input > 0 {
            accum.2 = real_input;
        }

        // Parse createdAt for speed calculation
        if let Some(ts_str) = b.get("createdAt").and_then(|t| t.as_str()) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                accum.1.push((dt.timestamp() as f64, estimated));
            }
        }
    }

    // Compute speed per composer
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    let window_start = now - 300.0;

    for (cid, (total_output, timed_tokens, last_input)) in &accums {
        let speed = if timed_tokens.len() >= 2 {
            let recent: Vec<_> = timed_tokens
                .iter()
                .filter(|(ts, _)| *ts > window_start)
                .collect();

            if recent.len() >= 2 {
                let total_recent: u64 = recent.iter().map(|(_, t)| *t).sum();
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

        if *total_output > 0 || speed > 0.0 || *last_input > 0 {
            let ctx = if *last_input > 0 { Some(*last_input) } else { None };
            result.insert(cid.clone(), (speed, *total_output, ctx));
        }
    }

    result
}

// ── Main scanner ─────────────────────────────────────────────────────────────

pub fn scan_cursor_sessions(_cursor_dir: &Path) -> Vec<SessionInfo> {
    let conn = match open_cursor_db() {
        Ok(c) => c,
        Err(_) => return vec![],
    };

    let mut stmt = match conn.prepare(
        "SELECT key, value FROM cursorDiskKV WHERE key LIKE 'composerData:%'"
    ) {
        Ok(s) => s,
        Err(_) => return vec![],
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let seven_days_ms = 7 * 24 * 3600 * 1000u64;

    // Track subagent IDs and parent mapping
    let mut subagent_ids: HashSet<String> = HashSet::new();
    let mut parent_map: HashMap<String, String> = HashMap::new();

    struct RawComposer {
        composer_id: String,
        name: Option<String>,
        status: String,
        model: Option<String>,
        created_at_ms: u64,
        last_updated_ms: u64,
        is_agentic: bool,
        #[allow(dead_code)]
        subagent_composer_ids: Vec<String>,
        subtitle: Option<String>,
    }

    let mut composers: Vec<RawComposer> = Vec::new();

    // Collect all values upfront to release the borrow on stmt/conn
    let all_values: Vec<String> = {
        let rows = match stmt.query_map([], |row| {
            let value: String = row.get(1)?;
            Ok(value)
        }) {
            Ok(r) => r,
            Err(_) => return vec![],
        };
        rows.flatten().collect()
    };
    drop(stmt);
    drop(conn);

    for row in &all_values {
        let Ok(v): Result<Value, _> = serde_json::from_str(row) else {
            continue;
        };

        let composer_id = v.get("composerId")
            .and_then(|c| c.as_str())
            .unwrap_or_default()
            .to_string();

        if composer_id.is_empty() {
            continue;
        }

        let created_at = v.get("createdAt")
            .and_then(|c| c.as_u64())
            .unwrap_or(0);

        let last_updated = v.get("lastUpdatedAt")
            .and_then(|c| c.as_u64())
            .unwrap_or(created_at);

        // Skip sessions older than 7 days
        if now_ms.saturating_sub(last_updated) > seven_days_ms {
            continue;
        }

        let name = v.get("name")
            .and_then(|n| n.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string());

        let status = v.get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("completed")
            .to_string();

        let model = extract_model_name(&v);

        let is_agentic = v.get("isAgentic")
            .and_then(|a| a.as_bool())
            .unwrap_or(false);

        let sub_ids: Vec<String> = v.get("subagentComposerIds")
            .and_then(|a| a.as_array())
            .map(|arr| arr.iter().filter_map(|id| id.as_str().map(|s| s.to_string())).collect())
            .unwrap_or_default();

        for sub_id in &sub_ids {
            subagent_ids.insert(sub_id.clone());
            parent_map.insert(sub_id.clone(), composer_id.clone());
        }

        let subtitle = v.get("subtitle")
            .and_then(|s| s.as_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(200).collect::<String>());

        composers.push(RawComposer {
            composer_id,
            name,
            status,
            model,
            created_at_ms: created_at,
            last_updated_ms: last_updated,
            is_agentic,
            subagent_composer_ids: sub_ids,
            subtitle,
        });
    }

    // Build workspace map from JSONL transcript directories
    let workspace_map = if let Some(cursor_dir) = get_cursor_dir() {
        build_workspace_map(&cursor_dir)
    } else {
        HashMap::new()
    };

    // Only estimate token stats for non-idle sessions (avoids hundreds of slow
    // LIKE queries against the 10 GB+ database for historical sessions).
    let active_ids: Vec<&str> = composers
        .iter()
        .filter(|c| {
            !matches!(
                map_cursor_status(&c.status, c.last_updated_ms),
                SessionStatus::Idle
            )
        })
        .map(|c| c.composer_id.as_str())
        .collect();
    let token_stats = estimate_cursor_token_stats(&active_ids);

    let mut sessions = Vec::new();

    for c in &composers {
        let is_subagent = subagent_ids.contains(&c.composer_id);
        let parent_id = parent_map.get(&c.composer_id).cloned();
        let session_status = map_cursor_status(&c.status, c.last_updated_ms);

        // Find workspace from transcript directory
        let (workspace_path, _workspace_encoded) = workspace_map
            .get(&c.composer_id)
            .cloned()
            .unwrap_or_else(|| ("(Cursor)".to_string(), String::new()));

        let ws_name = if workspace_path == "(Cursor)" {
            "Cursor".to_string()
        } else {
            workspace_name(&workspace_path)
        };

        // Use cursor:// URI as identifier (not a file path)
        let cursor_uri = format!("{}{}", CURSOR_URI_PREFIX, c.composer_id);

        let agent_type = if is_subagent {
            if c.is_agentic {
                Some("general-purpose".to_string())
            } else {
                Some("explore".to_string())
            }
        } else {
            None
        };

        sessions.push(SessionInfo {
            id: c.composer_id.clone(),
            workspace_path: workspace_path.clone(),
            workspace_name: ws_name,
            ide_name: Some("Cursor".to_string()),
            is_subagent,
            parent_session_id: parent_id,
            agent_type,
            agent_description: if is_subagent { c.name.clone() } else { None },
            slug: None,
            ai_title: if !is_subagent { c.name.clone() } else { None },
            status: session_status,
            token_speed: token_stats.get(&c.composer_id).map_or(0.0, |s| s.0),
            total_output_tokens: token_stats.get(&c.composer_id).map_or(0, |s| s.1),
            total_cost_usd: 0.0,
            agent_total_cost_usd: 0.0,
            cost_speed_usd_per_min: 0.0,
            last_message_preview: c.subtitle.clone().or_else(|| c.name.clone()),
            last_activity_ms: c.last_updated_ms,
            created_at_ms: c.created_at_ms,
            jsonl_path: cursor_uri,
            model: c.model.clone(),
            thinking_level: None,
            pid: None,
            pid_precise: false,
            last_skill: None,
            context_percent: token_stats
                .get(&c.composer_id)
                .and_then(|s| s.2)
                .and_then(|input| compute_context_percent(input, c.model.as_deref(), input)),
            agent_source: "cursor".to_string(),
            last_outcome: None,
            rate_limit: None,
        });
    }

    // Promote main sessions to Delegating if they have actively-working subagents.
    // A subagent that is WaitingInput has finished its turn and should not cause the parent
    // to show as Delegating — otherwise the parent's own WaitingInput status gets hidden.
    let active_parent_ids: HashSet<String> = sessions
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

    sessions
}

// ── Message reading from SQLite ──────────────────────────────────────────────

/// Read messages for a Cursor session from bubbleId entries in state.vscdb.
/// Returns messages in Claude Code JSONL-compatible format for the frontend.
pub fn get_cursor_messages(composer_id: &str) -> Result<Vec<Value>, String> {
    let conn = open_cursor_db()?;

    // First, read composerData to get the bubble ordering
    let composer_key = format!("composerData:{}", composer_id);
    let composer_json: String = conn
        .query_row(
            "SELECT value FROM cursorDiskKV WHERE key = ?1",
            [&composer_key],
            |row| row.get(0),
        )
        .map_err(|e| format!("composerData not found: {e}"))?;

    let composer: Value = serde_json::from_str(&composer_json)
        .map_err(|e| format!("Cannot parse composerData: {e}"))?;

    // Get ordered bubble IDs from fullConversationHeadersOnly
    let headers = composer
        .get("fullConversationHeadersOnly")
        .and_then(|h| h.as_array())
        .cloned()
        .unwrap_or_default();

    // Batch-fetch all bubble data in a single query instead of N individual queries.
    let bubble_prefix = format!("bubbleId:{}:", composer_id);
    let mut bubble_stmt = conn
        .prepare("SELECT key, value FROM cursorDiskKV WHERE key LIKE ?1")
        .map_err(|e| format!("Cannot prepare bubble query: {e}"))?;
    let bubble_map: HashMap<String, Value> = bubble_stmt
        .query_map([format!("{}%", bubble_prefix)], |row| {
            let key: String = row.get(0)?;
            let val: String = row.get(1)?;
            Ok((key, val))
        })
        .map_err(|e| format!("Cannot query bubbles: {e}"))?
        .flatten()
        .filter_map(|(key, val)| {
            let bubble_id = key.strip_prefix(&bubble_prefix)?.to_string();
            let parsed: Value = serde_json::from_str(&val).ok()?;
            Some((bubble_id, parsed))
        })
        .collect();

    let mut messages: Vec<Value> = Vec::new();

    for header in &headers {
        let bubble_id = header
            .get("bubbleId")
            .and_then(|b| b.as_str())
            .or_else(|| header.get("bubbleId").and_then(|b| b.as_u64()).map(|_| ""))
            .unwrap_or_default();

        let bubble_type = header
            .get("type")
            .and_then(|t| t.as_u64())
            .unwrap_or(0);

        if bubble_id.is_empty() && bubble_type == 0 {
            continue;
        }

        let bubble: &Value = match bubble_map.get(bubble_id) {
            Some(v) => v,
            None => continue,
        };

        // Map bubble to Claude Code-compatible message format
        let role = match bubble_type {
            1 => "user",
            2 => "assistant",
            _ => continue,
        };

        let mut content_blocks: Vec<Value> = Vec::new();

        // Add thinking block if present
        if let Some(thinking) = bubble.get("thinking").and_then(|t| t.get("text")).and_then(|t| t.as_str()) {
            if !thinking.is_empty() {
                content_blocks.push(serde_json::json!({
                    "type": "thinking",
                    "thinking": thinking
                }));
            }
        }

        // Add text content
        if let Some(text) = bubble.get("text").and_then(|t| t.as_str()) {
            if !text.is_empty() {
                content_blocks.push(serde_json::json!({
                    "type": "text",
                    "text": text
                }));
            }
        }

        // Add tool calls from toolFormerData
        if let Some(tools) = bubble.get("toolFormerData").and_then(|t| t.as_array()) {
            for tool in tools {
                let tool_name = tool.get("name").and_then(|n| n.as_str()).unwrap_or("unknown");
                let tool_status = tool.get("status").and_then(|s| s.as_str()).unwrap_or("");

                content_blocks.push(serde_json::json!({
                    "type": "tool_use",
                    "id": tool.get("id").and_then(|i| i.as_str()).unwrap_or(""),
                    "name": tool_name,
                    "input": tool.get("params").unwrap_or(&Value::Null),
                    "_cursor_status": tool_status
                }));
            }
        }

        if content_blocks.is_empty() {
            continue;
        }

        let created_at = bubble
            .get("createdAt")
            .and_then(|c| c.as_u64())
            .unwrap_or(0);

        let timestamp = if created_at > 0 {
            chrono::DateTime::from_timestamp_millis(created_at as i64)
                .map(|dt| dt.to_rfc3339())
                .unwrap_or_default()
        } else {
            String::new()
        };

        messages.push(serde_json::json!({
            "type": role,
            "timestamp": timestamp,
            "message": {
                "role": role,
                "content": content_blocks,
                "stop_reason": if role == "assistant" { "end_turn" } else { serde_json::json!(null).as_str().unwrap_or("") },
            }
        }));
    }

    Ok(messages)
}

// ── Workspace mapping ────────────────────────────────────────────────────────

/// Build a map from composerId → (workspace_path, workspace_encoded)
/// by scanning the JSONL transcript directory structure.
fn build_workspace_map(cursor_dir: &Path) -> HashMap<String, (String, String)> {
    let mut map = HashMap::new();
    let projects_dir = cursor_dir.join("projects");

    let Ok(workspace_entries) = std::fs::read_dir(&projects_dir) else {
        return map;
    };

    for ws_entry in workspace_entries.flatten() {
        let ws_dir = ws_entry.path();
        if !ws_dir.is_dir() {
            continue;
        }

        let encoded = ws_dir
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default()
            .to_string();

        let workspace_path = decode_workspace_path(&encoded);

        let transcripts_dir = ws_dir.join("agent-transcripts");
        if !transcripts_dir.is_dir() {
            continue;
        }

        let Ok(transcript_entries) = std::fs::read_dir(&transcripts_dir) else {
            continue;
        };

        for t_entry in transcript_entries.flatten() {
            let t_dir = t_entry.path();
            if !t_dir.is_dir() {
                continue;
            }

            let composer_id = t_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();

            if !composer_id.is_empty() {
                map.insert(composer_id.clone(), (workspace_path.clone(), encoded.clone()));

                // Also scan subagents directory
                let subagents_dir = t_dir.join("subagents");
                if subagents_dir.is_dir() {
                    if let Ok(sub_entries) = std::fs::read_dir(&subagents_dir) {
                        for sub_entry in sub_entries.flatten() {
                            let sub_path = sub_entry.path();
                            if sub_path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                                let sub_id = sub_path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or_default()
                                    .to_string();
                                if !sub_id.is_empty() {
                                    map.insert(sub_id, (workspace_path.clone(), encoded.clone()));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    map
}

// ── Cursor account & usage info ──────────────────────────────────────────────

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CursorDailyStats {
    pub date: String,
    pub tab_suggested_lines: u64,
    pub tab_accepted_lines: u64,
    pub composer_suggested_lines: u64,
    pub composer_accepted_lines: u64,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CursorUsageItem {
    pub name: String,
    pub used: u64,
    pub limit: Option<u64>,
    /// Utilization as a 0–1 fraction (from usage-summary API).
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, Default)]
#[serde(rename_all = "camelCase")]
pub struct CursorAccountInfo {
    pub email: String,
    pub sign_up_type: String,
    pub membership_type: String,
    pub subscription_status: String,
    pub total_prompts: u64,
    pub daily_stats: Vec<CursorDailyStats>,
    pub usage: Vec<CursorUsageItem>,
}

/// Read a single key from the ItemTable in state.vscdb.
fn read_item(conn: &rusqlite::Connection, key: &str) -> Option<String> {
    conn.query_row(
        "SELECT value FROM ItemTable WHERE key = ?1",
        [key],
        |row| row.get::<_, String>(0),
    )
    .ok()
}

/// Stripe profile response from api2.cursor.sh/auth/full_stripe_profile.
/// Only fields we actually use are kept; the rest are ignored via default serde behavior.
#[derive(serde::Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct StripeProfile {
    membership_type: Option<String>,
    subscription_status: Option<String>,
    individual_membership_type: Option<String>,
}

/// Fetch real-time Stripe profile from Cursor API.
async fn fetch_stripe_profile(access_token: &str) -> Result<StripeProfile, String> {
    let client = reqwest::Client::new();

    let resp = client
        .get("https://api2.cursor.sh/auth/full_stripe_profile")
        .header("Authorization", format!("Bearer {}", access_token))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("Cursor profile request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Cursor profile API returned {}", resp.status()));
    }

    resp.json::<StripeProfile>()
        .await
        .map_err(|e| format!("Cannot parse Cursor profile: {e}"))
}

/// Fetch aggregated usage from api2.cursor.sh/auth/usage-summary.
///
/// This endpoint returns plan-level data with utilization percentages,
/// premium vs auto breakdown, and billing cycle dates.
async fn fetch_cursor_usage_api(access_token: &str) -> Result<Vec<CursorUsageItem>, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get("https://api2.cursor.sh/auth/usage-summary")
        .header("Authorization", format!("Bearer {}", access_token))
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("Cursor usage-summary request failed: {e}"))?;

    if !resp.status().is_success() {
        return Err(format!("Cursor usage-summary API returned {}", resp.status()));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("Cannot parse Cursor usage-summary response: {e}"))?;

    parse_usage_summary(&body)
}

/// Parse the usage-summary response into CursorUsageItems.
fn parse_usage_summary(body: &serde_json::Value) -> Result<Vec<CursorUsageItem>, String> {
    let billing_end = body
        .get("billingCycleEnd")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let plan = body.pointer("/individualUsage/plan");

    let mut items = Vec::new();

    if let Some(plan_obj) = plan {
        let used = plan_obj.get("used").and_then(|v| v.as_u64()).unwrap_or(0);
        let limit = plan_obj.get("limit").and_then(|v| v.as_u64());

        // Auto (total - API = auto portion)
        let auto_pct = plan_obj.get("autoPercentUsed").and_then(|v| v.as_f64());
        let api_pct = plan_obj.get("apiPercentUsed").and_then(|v| v.as_f64());
        let total_pct = plan_obj.get("totalPercentUsed").and_then(|v| v.as_f64());

        // Show premium (API / named models) usage
        if let Some(pct) = api_pct {
            items.push(CursorUsageItem {
                name: "Premium".to_string(),
                used: 0, // individual count not available in summary
                limit: None,
                utilization: Some(pct / 100.0),
                resets_at: billing_end.clone(),
            });
        }

        // Show auto usage
        if let Some(pct) = auto_pct {
            items.push(CursorUsageItem {
                name: "Auto".to_string(),
                used: 0,
                limit: None,
                utilization: Some(pct / 100.0),
                resets_at: billing_end.clone(),
            });
        }

        // Show total plan usage if we have it and both sub-categories
        if api_pct.is_some() && auto_pct.is_some() {
            if let Some(pct) = total_pct {
                items.push(CursorUsageItem {
                    name: "Total".to_string(),
                    used,
                    limit,
                    utilization: Some(pct / 100.0),
                    resets_at: billing_end.clone(),
                });
            }
        }
    }

    // On-demand usage
    if let Some(od) = body.pointer("/individualUsage/onDemand") {
        let used = od.get("used").and_then(|v| v.as_u64()).unwrap_or(0);
        let limit = od.get("limit").and_then(|v| v.as_u64());
        if used > 0 || limit.is_some() {
            let utilization = match (limit, used) {
                (Some(l), u) if l > 0 => Some(u as f64 / l as f64),
                _ => None,
            };
            items.push(CursorUsageItem {
                name: "On-demand".to_string(),
                used,
                limit,
                utilization,
                resets_at: billing_end,
            });
        }
    }

    Ok(items)
}

pub async fn fetch_cursor_account_info() -> Result<CursorAccountInfo, String> {
    // Read local data from SQLite (fast — sub-millisecond, safe to do inline).
    let conn = open_cursor_db()?;

    let email = read_item(&conn, "cursorAuth/cachedEmail").unwrap_or_default();
    if email.is_empty() {
        return Err("Not logged in to Cursor IDE".to_string());
    }

    let sign_up_type = read_item(&conn, "cursorAuth/cachedSignUpType").unwrap_or_default();
    let mut membership_type = read_item(&conn, "cursorAuth/stripeMembershipType").unwrap_or_default();
    let mut subscription_status = read_item(&conn, "cursorAuth/stripeSubscriptionStatus").unwrap_or_default();
    let total_prompts = read_item(&conn, "freeBestOfN.promptCount")
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    let access_token = read_item(&conn, "cursorAuth/accessToken");

    let daily_stats: Vec<CursorDailyStats> = {
        let mut stats = Vec::new();
        let mut stmt = conn
            .prepare("SELECT value FROM ItemTable WHERE key LIKE 'aiCodeTracking.dailyStats%' ORDER BY key DESC LIMIT 7")
            .map_err(|e| format!("Cannot query daily stats: {e}"))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Cannot read daily stats: {e}"))?;
        for row in rows.flatten() {
            if let Ok(s) = serde_json::from_str::<CursorDailyStats>(&row) {
                stats.push(s);
            }
        }
        stats
    };

    // Drop DB connection before async network calls
    drop(conn);

    // Fetch real-time data from API concurrently (non-blocking async HTTP)
    if let Some(ref token) = access_token {
        let profile_fut = fetch_stripe_profile(token);
        let usage_fut = fetch_cursor_usage_api(token);

        // Run both API calls concurrently
        let (profile_res, usage_res) = futures::future::join(profile_fut, usage_fut).await;

        if let Ok(profile) = profile_res {
            membership_type = profile
                .individual_membership_type
                .or(profile.membership_type)
                .unwrap_or(membership_type);
            subscription_status = profile.subscription_status.unwrap_or(subscription_status);
        } else {
            crate::log_debug("Cursor stripe profile API failed, using cached data");
        }

        let usage = match usage_res {
            Ok(items) => items,
            Err(e) => {
                crate::log_debug(&format!("Cursor usage API error: {e}"));
                vec![]
            }
        };

        return Ok(CursorAccountInfo {
            email,
            sign_up_type,
            membership_type,
            subscription_status,
            total_prompts,
            daily_stats,
            usage,
        });
    }

    Ok(CursorAccountInfo {
        email,
        sign_up_type,
        membership_type,
        subscription_status,
        total_prompts,
        daily_stats,
        usage: vec![],
    })
}

/// Blocking wrapper for `fetch_cursor_account_info` (for use in trait methods).
/// Handles being called both from within a tokio runtime (via `block_in_place`)
/// and from plain threads (via a new runtime).
pub fn fetch_cursor_account_info_blocking() -> Result<CursorAccountInfo, String> {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(fetch_cursor_account_info()))
    } else {
        tokio::runtime::Runtime::new()
            .map_err(|e| format!("failed to create tokio runtime: {e}"))?
            .block_on(fetch_cursor_account_info())
    }
}

// ── AgentSource implementation ──────────────────────────────────────────────

use crate::agent_source::{AgentSource, WatchStrategy};
use crate::backend::SourceUsageSummary;

pub struct CursorSource;

impl AgentSource for CursorSource {
    fn name(&self) -> &'static str {
        "cursor"
    }

    fn uri_prefix(&self) -> &'static str {
        CURSOR_URI_PREFIX
    }

    fn is_available(&self) -> bool {
        state_vscdb_path().map(|p| p.exists()).unwrap_or(false)
    }

    fn scan_sessions(&self) -> Vec<SessionInfo> {
        let cursor_dir = match get_cursor_dir() {
            Some(d) => d,
            None => return vec![],
        };
        scan_cursor_sessions(&cursor_dir)
    }

    fn get_messages(&self, path: &str) -> Result<Vec<serde_json::Value>, String> {
        let composer_id = path
            .strip_prefix(CURSOR_URI_PREFIX)
            .ok_or_else(|| format!("Invalid cursor URI: {path}"))?;
        get_cursor_messages(composer_id)
    }

    fn resolve_file_path(&self, _path: &str) -> Option<std::path::PathBuf> {
        None // Cursor uses SQLite, no JSONL file to tail
    }

    fn watch_strategy(&self) -> WatchStrategy {
        WatchStrategy::Poll(std::time::Duration::from_secs(5))
    }

    // Cursor uses SQLite polling, no filesystem paths to watch.

    fn fetch_account(&self) -> Result<serde_json::Value, String> {
        let info = fetch_cursor_account_info_blocking()?;
        serde_json::to_value(&info).map_err(|e| e.to_string())
    }

    fn fetch_usage(&self) -> Result<serde_json::Value, String> {
        // Cursor account info includes usage data (requests remaining etc.)
        self.fetch_account()
    }

    fn usage_summary(&self) -> Option<SourceUsageSummary> {
        let info = fetch_cursor_account_info_blocking().ok()?;
        let val = serde_json::to_value(&info).ok()?;
        Some(SourceUsageSummary::from_cursor(&val))
    }
}
