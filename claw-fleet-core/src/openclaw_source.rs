//! OpenClaw agent source — scans ~/.openclaw/agents/ for JSONL session files.
//!
//! OpenClaw stores sessions under `~/.openclaw/agents/<agentId>/sessions/`:
//!   - `sessions.json` maps session keys to session IDs
//!   - `<session-id>.jsonl` contains the conversation transcript
//!
//! The JSONL format is similar to Claude Code's (role, content, timestamp, tool_use),
//! so we can reuse much of the shared parsing infrastructure.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent_source::{AgentSource, WatchStrategy};
use crate::backend::SourceUsageSummary;
use crate::session::{SessionInfo, SessionStatus, extract_last_context_usage, compute_context_percent};

/// URI prefix for OpenClaw session identifiers.
const OPENCLAW_URI_PREFIX: &str = "openclaw://";

pub struct OpenClawSource {
    process_cache: std::sync::Mutex<(std::time::Instant, Vec<(u32, String)>)>,
    _session_cache: std::sync::Mutex<std::collections::HashMap<String, (u64, SessionInfo)>>,
}

impl OpenClawSource {
    pub fn new() -> Self {
        Self {
            process_cache: std::sync::Mutex::new((
                std::time::Instant::now() - std::time::Duration::from_secs(999),
                Vec::new(),
            )),
            _session_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn get_openclaw_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".openclaw"))
}

fn get_agents_dir() -> Option<PathBuf> {
    get_openclaw_dir().map(|d| d.join("agents"))
}

/// Resolve an `openclaw://` URI to the actual JSONL file path.
/// Format: `openclaw://<agent-id>/<session-id>`
fn resolve_uri(uri: &str) -> Option<PathBuf> {
    let stripped = uri.strip_prefix(OPENCLAW_URI_PREFIX)?;
    let mut parts = stripped.splitn(2, '/');
    let agent_id = parts.next()?;
    let session_id = parts.next()?;

    let agents_dir = get_agents_dir()?;
    Some(
        agents_dir
            .join(agent_id)
            .join("sessions")
            .join(format!("{}.jsonl", session_id)),
    )
}

/// Extract the last assistant text block from parsed JSONL lines.
fn extract_last_text(lines: &[Value]) -> Option<String> {
    for msg in lines.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let content = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())?;
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

/// Determine session status from the last JSONL lines and file age.
/// Simplified version of Claude Code's determine_status (no hooks support).
fn determine_status(last_lines: &[Value], file_age_secs: f64) -> SessionStatus {
    if file_age_secs < 8.0 {
        // Check for streaming content.
        let last_partial = last_lines.iter().rev().find(|v| {
            if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
                return false;
            }
            let stop = v.get("message").and_then(|m| m.get("stop_reason"));
            stop.map_or(true, |s| s.is_null())
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
    }

    // Check the last complete message.
    let last_meaningful = last_lines.iter().rev().find(|v| {
        matches!(
            v.get("type").and_then(|t| t.as_str()),
            Some("user") | Some("assistant")
        )
    });

    if let Some(last) = last_meaningful {
        let last_type = last.get("type").and_then(|t| t.as_str());

        if last_type == Some("user") && file_age_secs < 120.0 {
            return SessionStatus::Thinking;
        }

        if last_type == Some("assistant") {
            let stop_value = last.get("message").and_then(|m| m.get("stop_reason"));
            let stop_reason = stop_value.and_then(|s| s.as_str());
            let stop_is_null = stop_value.map_or(true, |s| s.is_null());

            if stop_is_null && file_age_secs < 120.0 {
                return SessionStatus::Streaming;
            }

            match stop_reason {
                Some("end_turn") if file_age_secs < 300.0 => return SessionStatus::WaitingInput,
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

/// Compute token speed and total output tokens from JSONL lines.
fn compute_token_stats(lines: &[&str]) -> (f64, u64) {
    let mut total_output: u64 = 0;
    let mut timed_tokens: Vec<(f64, u64)> = Vec::new();
    let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

    for line in lines {
        let Ok(v): Result<Value, _> = serde_json::from_str(line) else { continue };
        if v.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(msg) = v.get("message").and_then(|m| m.as_object()) else { continue };
        if msg.get("stop_reason").map_or(true, |s| s.is_null()) {
            continue;
        }
        let msg_id = msg.get("id").and_then(|i| i.as_str()).unwrap_or_default().to_string();
        if !msg_id.is_empty() {
            if seen_ids.contains(&msg_id) { continue; }
            seen_ids.insert(msg_id);
        }

        let output_tokens = msg
            .get("usage")
            .and_then(|u| u.get("output_tokens"))
            .and_then(|t| t.as_u64())
            .unwrap_or(0);
        total_output += output_tokens;

        if let Some(ts_str) = v.get("timestamp").and_then(|t| t.as_str()) {
            if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(ts_str) {
                timed_tokens.push((dt.timestamp() as f64, output_tokens));
            }
        }
    }

    let speed = if timed_tokens.len() >= 2 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs_f64();
        let window_start = now - 300.0;
        let recent: Vec<_> = timed_tokens.iter().filter(|(ts, _)| *ts > window_start).collect();
        if recent.len() >= 2 {
            let total_recent: u64 = recent.iter().map(|(_, t)| t).sum();
            let first_ts = recent.first().map(|(ts, _)| *ts).unwrap_or(0.0);
            let last_ts = recent.last().map(|(ts, _)| *ts).unwrap_or(0.0);
            let duration = last_ts - first_ts;
            if duration > 0.0 { total_recent as f64 / duration } else { 0.0 }
        } else {
            0.0
        }
    } else {
        0.0
    };

    (speed, total_output)
}

/// Extract model name from assistant messages.
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
        if !model.is_empty() && model != "unknown" {
            return Some(model.to_string());
        }
    }
    None
}

/// Scan OpenClaw processes to find PIDs.
fn scan_openclaw_processes() -> Vec<(u32, String)> {
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
            let cmd_str: String = p.cmd().iter()
                .map(|s| s.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join(" ");
            name == "openclaw" || name == "openclaw.exe"
                || (name.starts_with("node") && cmd_str.contains("openclaw"))
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
                    result.push((pid.as_u32(), path.to_string()));
                }
            }
        }
    }
    result
}

/// Scan a single session JSONL file into a SessionInfo.
fn parse_openclaw_session(
    jsonl_path: &Path,
    agent_id: &str,
    session_id: &str,
    openclaw_processes: &[(u32, String)],
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

    // Skip sessions older than 7 days.
    if age > Duration::from_secs(7 * 24 * 3600) {
        return None;
    }

    let content = fs::read_to_string(jsonl_path).ok()?;
    let all_lines: Vec<&str> = content.lines().collect();

    let start = all_lines.len().saturating_sub(100);
    let last_n: Vec<Value> = all_lines[start..]
        .iter()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();

    let status = determine_status(&last_n, age.as_secs_f64());
    let (token_speed, total_output_tokens) = compute_token_stats(&all_lines);
    let last_message_preview = extract_last_text(&last_n);
    let model = extract_model(&last_n);

    // Try to extract workspace path from the agent's config or session metadata.
    // Fallback: use the agent directory as the "workspace".
    let workspace_path = get_agents_dir()
        .map(|d| d.join(agent_id).to_string_lossy().to_string())
        .unwrap_or_else(|| format!("openclaw/{}", agent_id));

    let workspace_name = format!("openclaw/{}", agent_id);

    // AI title: look for an "ai-title" entry in the JSONL.
    let ai_title = all_lines
        .iter()
        .filter_map(|l| serde_json::from_str::<Value>(l).ok())
        .find(|v| v.get("type").and_then(|t| t.as_str()) == Some("ai-title"))
        .and_then(|v| v.get("aiTitle").and_then(|t| t.as_str()).map(|s| s.to_string()));

    // Session slug
    let slug = last_n
        .iter()
        .filter_map(|v| v.get("slug").and_then(|s| s.as_str()).map(|s| s.to_string()))
        .last();

    // Thinking level detection
    let has_thinking = last_n.iter().any(|v| {
        v.get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
            .map_or(false, |blocks| {
                blocks.iter().any(|b| b.get("type").and_then(|t| t.as_str()) == Some("thinking"))
            })
    });
    let thinking_level = if has_thinking { Some("thinking".to_string()) } else { None };

    // PID resolution: match openclaw processes by workspace path.
    let pid = openclaw_processes
        .iter()
        .find(|(_, cwd)| cwd == &workspace_path)
        .map(|(pid, _)| *pid);

    let uri = format!("{}{}/{}", OPENCLAW_URI_PREFIX, agent_id, session_id);

    Some(SessionInfo {
        id: session_id.to_string(),
        workspace_path,
        workspace_name,
        ide_name: None,
        is_subagent: false,
        parent_session_id: None,
        agent_type: None,
        agent_description: None,
        slug,
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
        pid_precise: pid.is_some(),
        last_skill: None,
        context_percent: extract_last_context_usage(&all_lines)
            .and_then(|(used, m, max)| compute_context_percent(used, Some(&m), max)),
        agent_source: "openclaw".to_string(),
        last_outcome: None,
        rate_limit: None,
        todos: None,
        compact_count: 0,
        compact_pre_tokens: 0,
        compact_post_tokens: 0,
        compact_cost_usd: 0.0,
    })
}

/// Read the sessions.json index for an agent.
fn read_sessions_index(agent_dir: &Path) -> HashMap<String, String> {
    let index_path = agent_dir.join("sessions").join("sessions.json");
    let Ok(content) = fs::read_to_string(&index_path) else {
        return HashMap::new();
    };
    let Ok(parsed) = serde_json::from_str::<Value>(&content) else {
        return HashMap::new();
    };
    let mut map = HashMap::new();
    if let Some(obj) = parsed.as_object() {
        for (key, val) in obj {
            if let Some(id) = val.as_str() {
                map.insert(key.clone(), id.to_string());
            }
        }
    }
    map
}

// ── AgentSource implementation ──────────────────────────────────────────────

impl AgentSource for OpenClawSource {
    fn name(&self) -> &'static str {
        "openclaw"
    }

    fn uri_prefix(&self) -> &'static str {
        OPENCLAW_URI_PREFIX
    }

    fn is_available(&self) -> bool {
        get_openclaw_dir().map(|d| d.is_dir()).unwrap_or(false)
    }

    fn scan_sessions(&self) -> Vec<SessionInfo> {
        let Some(agents_dir) = get_agents_dir() else {
            return vec![];
        };
        if !agents_dir.is_dir() {
            return vec![];
        }

        // Reuse cached process list if fresh (< 10 s).
        let openclaw_processes = {
            let mut guard = self.process_cache.lock().unwrap();
            if guard.0.elapsed() > Duration::from_secs(10) {
                guard.1 = scan_openclaw_processes();
                guard.0 = std::time::Instant::now();
            }
            guard.1.clone()
        };
        let mut sessions = Vec::new();

        let Ok(agent_entries) = fs::read_dir(&agents_dir) else {
            return vec![];
        };

        for agent_entry in agent_entries.flatten() {
            let agent_dir = agent_entry.path();
            if !agent_dir.is_dir() {
                continue;
            }

            let agent_id = agent_dir
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .to_string();

            let sessions_dir = agent_dir.join("sessions");
            if !sessions_dir.is_dir() {
                continue;
            }

            // Read session index to get session IDs.
            let session_index = read_sessions_index(&agent_dir);

            // Also scan for any .jsonl files directly (in case sessions.json is incomplete).
            let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

            // Process sessions from the index first.
            for (_key, session_id) in &session_index {
                let jsonl_path = sessions_dir.join(format!("{}.jsonl", session_id));
                if jsonl_path.is_file() {
                    if let Some(info) = parse_openclaw_session(
                        &jsonl_path,
                        &agent_id,
                        session_id,
                        &openclaw_processes,
                    ) {
                        sessions.push(info);
                        seen_ids.insert(session_id.clone());
                    }
                }
            }

            // Also scan for .jsonl files not in the index.
            if let Ok(entries) = fs::read_dir(&sessions_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    let session_id = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                        .to_string();
                    if seen_ids.contains(&session_id) {
                        continue;
                    }
                    if let Some(info) = parse_openclaw_session(
                        &path,
                        &agent_id,
                        &session_id,
                        &openclaw_processes,
                    ) {
                        sessions.push(info);
                    }
                }
            }
        }

        sessions
    }

    fn get_messages(&self, path: &str) -> Result<Vec<Value>, String> {
        let file_path = resolve_uri(path)
            .ok_or_else(|| format!("Invalid OpenClaw URI: {path}"))?;

        let content = fs::read_to_string(&file_path)
            .map_err(|e| format!("Cannot read OpenClaw session: {e}"))?;

        // Parse JSONL lines. OpenClaw uses a similar format to Claude Code.
        // If the format differs, normalize here.
        let messages: Vec<Value> = content
            .lines()
            .filter(|l| !l.trim().is_empty())
            .filter_map(|l| {
                let v: Value = serde_json::from_str(l).ok()?;
                // Normalize OpenClaw-specific fields to match the common format.
                Some(normalize_message(v))
            })
            .collect();

        Ok(messages)
    }

    fn get_messages_tail(&self, path: &str, n: usize) -> Result<Vec<Value>, String> {
        let file_path = resolve_uri(path)
            .ok_or_else(|| format!("Invalid OpenClaw URI: {path}"))?;
        let parsed = crate::jsonl_tail::read_tail_lines_as_json(&file_path, n)
            .map_err(|e| format!("Cannot read OpenClaw session: {e}"))?;
        Ok(parsed.into_iter().map(normalize_message).collect())
    }

    fn resolve_file_path(&self, path: &str) -> Option<std::path::PathBuf> {
        resolve_uri(path)
    }

    fn watch_strategy(&self) -> WatchStrategy {
        WatchStrategy::Filesystem
    }

    fn watch_paths(&self) -> Vec<PathBuf> {
        get_agents_dir().into_iter().filter(|d| d.is_dir()).collect()
    }

    fn trigger_extensions(&self) -> Vec<&'static str> {
        vec!["jsonl", "json"]
    }

    fn kill_pid(&self, pid: u32) -> Result<(), String> {
        crate::session::kill_pid_impl(pid)
    }

    fn kill_workspace(&self, _workspace_path: &str) -> Result<(), String> {
        // OpenClaw doesn't use workspace-based process grouping.
        // Kill by PID instead.
        Err("OpenClaw: use kill_pid instead of kill_workspace".to_string())
    }

    fn fetch_account(&self) -> Result<Value, String> {
        let info = fetch_openclaw_account_blocking()?;
        serde_json::to_value(&info).map_err(|e| e.to_string())
    }

    fn fetch_usage(&self) -> Result<Value, String> {
        let info = fetch_openclaw_usage_blocking()?;
        serde_json::to_value(&info).map_err(|e| e.to_string())
    }

    fn usage_summary(&self) -> Option<SourceUsageSummary> {
        let info = fetch_openclaw_usage_blocking().ok()?;
        let val = serde_json::to_value(&info).ok()?;
        Some(SourceUsageSummary::from_openclaw(&val))
    }
}

/// Normalize an OpenClaw message to match the common Fleet format.
///
/// OpenClaw's JSONL is largely compatible with Claude Code's format.
/// This function handles any known differences:
/// - Different field names for role/content
/// - Missing type fields
/// - Tool call format differences
fn normalize_message(mut v: Value) -> Value {
    // If the message already has a "type" field matching our format, pass through.
    if v.get("type").and_then(|t| t.as_str()).is_some() {
        return v;
    }

    // OpenClaw may use "role" at the top level instead of "type".
    if let Some(role) = v.get("role").and_then(|r| r.as_str()).map(|s| s.to_string()) {
        v.as_object_mut().map(|obj| {
            obj.insert("type".to_string(), Value::String(role.clone()));
            // Wrap content in a "message" envelope if not present.
            if obj.get("message").is_none() {
                let mut message = serde_json::Map::new();
                message.insert("role".to_string(), Value::String(role));
                if let Some(content) = obj.remove("content") {
                    message.insert("content".to_string(), content);
                }
                if let Some(model) = obj.remove("model") {
                    message.insert("model".to_string(), model);
                }
                if let Some(stop) = obj.remove("stop_reason") {
                    message.insert("stop_reason".to_string(), stop);
                }
                if let Some(usage) = obj.remove("usage") {
                    message.insert("usage".to_string(), usage);
                }
                obj.insert("message".to_string(), Value::Object(message));
            }
        });
    }

    v
}

// ── OpenClaw account / usage ────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawProviderInfo {
    pub provider: String,
    pub auth_type: String,   // "token", "oauth", "apiKey"
    pub status: String,      // "ok", "static", "expired", etc.
    pub label: String,
    pub expires_at: Option<i64>,
    pub remaining_ms: Option<i64>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawAccountInfo {
    pub version: String,
    pub default_model: String,
    pub providers: Vec<OpenClawProviderInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawSessionUsage {
    pub session_id: String,
    pub agent_id: String,
    pub model: String,
    pub context_tokens: u64,
    pub total_tokens: Option<u64>,
    pub percent_used: Option<f64>,
    pub age_secs: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OpenClawUsageInfo {
    pub sessions: Vec<OpenClawSessionUsage>,
}

fn find_openclaw_binary() -> Option<PathBuf> {
    let home = crate::session::real_home_dir();

    // Check well-known installation paths first (GUI apps often lack full PATH).
    let mut candidates: Vec<PathBuf> = vec![
        PathBuf::from("/opt/homebrew/bin/openclaw"),   // Homebrew (Apple Silicon)
        PathBuf::from("/usr/local/bin/openclaw"),      // Homebrew (Intel) / manual
    ];
    if let Some(ref h) = home {
        // npm/pnpm/yarn global installs
        candidates.push(h.join(".local/bin/openclaw"));
        candidates.push(h.join(".npm-global/bin/openclaw"));
        // cargo install
        candidates.push(h.join(".cargo/bin/openclaw"));
    }

    for p in &candidates {
        if p.is_file() {
            return Some(p.clone());
        }
    }

    // Fallback: `which` / `where` (works in terminal-launched apps)
    #[cfg(unix)]
    let cmd = "which";
    #[cfg(not(unix))]
    let cmd = "where";

    if let Ok(output) = std::process::Command::new(cmd).arg("openclaw").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Some(PathBuf::from(path));
            }
        }
    }
    None
}

/// Build an augmented PATH that includes common Node.js installation directories.
/// GUI apps (like Tauri) often lack the full shell PATH, which causes `#!/usr/bin/env node`
/// scripts (like openclaw) to fail with exit code 127.
fn augmented_path() -> String {
    let mut dirs: Vec<String> = vec![
        "/opt/homebrew/bin".to_string(),
        "/usr/local/bin".to_string(),
    ];
    if let Some(home) = crate::session::real_home_dir() {
        let h = home.display().to_string();
        // nvm
        if let Ok(nvm_dir) = std::env::var("NVM_DIR") {
            // Try to find the default node version
            let default_path = std::path::PathBuf::from(&nvm_dir).join("alias/default");
            if let Ok(version) = std::fs::read_to_string(&default_path) {
                let version = version.trim();
                let nvm_bin = format!("{nvm_dir}/versions/node/v{version}/bin");
                if std::path::Path::new(&nvm_bin).is_dir() {
                    dirs.push(nvm_bin);
                }
            }
            // Also try current symlink
            let current = format!("{nvm_dir}/current/bin");
            if std::path::Path::new(&current).is_dir() {
                dirs.push(current);
            }
        }
        // fnm
        dirs.push(format!("{h}/Library/Application Support/fnm/aliases/default/bin"));
        dirs.push(format!("{h}/.local/share/fnm/aliases/default/bin"));
        // volta
        dirs.push(format!("{h}/.volta/bin"));
        // Common global install paths
        dirs.push(format!("{h}/.local/bin"));
        dirs.push(format!("{h}/.npm-global/bin"));
        dirs.push(format!("{h}/.cargo/bin"));
    }
    // Append the existing PATH so we don't lose anything
    if let Ok(existing) = std::env::var("PATH") {
        dirs.push(existing);
    }
    dirs.join(":")
}

/// Fetch OpenClaw account info (providers, model, version) via `openclaw models status --json`.
pub async fn fetch_openclaw_account_info() -> Result<OpenClawAccountInfo, String> {
    let bin = find_openclaw_binary().ok_or("OpenClaw binary not found")?;
    tokio::task::spawn_blocking(move || fetch_openclaw_account_blocking_impl(&bin))
        .await
        .map_err(|e| format!("join error: {e}"))?
}

/// Blocking wrapper for use in fleet CLI (no tauri async runtime).
pub fn fetch_openclaw_account_blocking() -> Result<OpenClawAccountInfo, String> {
    let bin = find_openclaw_binary().ok_or("OpenClaw binary not found")?;
    fetch_openclaw_account_blocking_impl(&bin)
}

fn fetch_openclaw_account_blocking_impl(bin: &Path) -> Result<OpenClawAccountInfo, String> {
    let output = std::process::Command::new(bin)
        .args(["models", "status", "--json"])
        .env("PATH", augmented_path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Failed to run openclaw models status: {e}"))?;

    if !output.status.success() {
        return Err(format!(
            "openclaw models status exited with {}",
            output.status
        ));
    }

    let body: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse models status JSON: {e}"))?;

    let default_model = body
        .get("defaultModel")
        .or_else(|| body.get("resolvedDefault"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();

    // Extract provider info from auth.oauth.profiles
    let mut providers = Vec::new();
    if let Some(profiles) = body
        .pointer("/auth/oauth/profiles")
        .and_then(|v| v.as_array())
    {
        for p in profiles {
            let provider = p
                .get("provider")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let auth_type = p
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let status = p
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let label = p
                .get("label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let expires_at = p.get("expiresAt").and_then(|v| v.as_i64());
            let remaining_ms = p.get("remainingMs").and_then(|v| v.as_i64());

            if !provider.is_empty() {
                providers.push(OpenClawProviderInfo {
                    provider,
                    auth_type,
                    status,
                    label,
                    expires_at,
                    remaining_ms,
                });
            }
        }
    }

    // Get version from the CLI
    let version = std::process::Command::new(bin)
        .args(["--version"])
        .env("PATH", augmented_path())
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();

    Ok(OpenClawAccountInfo {
        version,
        default_model,
        providers,
    })
}

/// Fetch OpenClaw usage info (session token counts) via `openclaw status --json`.
pub async fn fetch_openclaw_usage() -> Result<OpenClawUsageInfo, String> {
    let bin = find_openclaw_binary().ok_or("OpenClaw binary not found")?;
    tokio::task::spawn_blocking(move || fetch_openclaw_usage_blocking_impl(&bin))
        .await
        .map_err(|e| format!("join error: {e}"))?
}

/// Blocking wrapper for use in fleet CLI (no tauri async runtime).
pub fn fetch_openclaw_usage_blocking() -> Result<OpenClawUsageInfo, String> {
    let bin = find_openclaw_binary().ok_or("OpenClaw binary not found")?;
    fetch_openclaw_usage_blocking_impl(&bin)
}

fn fetch_openclaw_usage_blocking_impl(bin: &Path) -> Result<OpenClawUsageInfo, String> {
    let output = std::process::Command::new(bin)
        .args(["status", "--json"])
        .env("PATH", augmented_path())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output()
        .map_err(|e| format!("Failed to run openclaw status: {e}"))?;

    if !output.status.success() {
        return Err(format!("openclaw status exited with {}", output.status));
    }

    let body: Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("Failed to parse status JSON: {e}"))?;

    let mut sessions = Vec::new();
    if let Some(recent) = body
        .pointer("/sessions/recent")
        .and_then(|v| v.as_array())
    {
        for s in recent {
            let session_id = s
                .get("sessionId")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let agent_id = s
                .get("agentId")
                .and_then(|v| v.as_str())
                .unwrap_or("main")
                .to_string();
            let model = s
                .get("model")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            let context_tokens = s
                .get("contextTokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let total_tokens = s.get("totalTokens").and_then(|v| v.as_u64());
            let percent_used = s.get("percentUsed").and_then(|v| v.as_f64());
            let age_secs = s
                .get("age")
                .and_then(|v| v.as_f64())
                .map(|ms| ms / 1000.0)
                .unwrap_or(0.0);

            if !session_id.is_empty() {
                sessions.push(OpenClawSessionUsage {
                    session_id,
                    agent_id,
                    model,
                    context_tokens,
                    total_tokens,
                    percent_used,
                    age_secs,
                });
            }
        }
    }

    Ok(OpenClawUsageInfo { sessions })
}
