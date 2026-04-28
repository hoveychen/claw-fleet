//! Guard — real-time interception of critical Bash commands via Claude Code
//! synchronous `PreToolUse` hooks.
//!
//! When a Critical-risk command is detected, the `fleet guard` CLI subprocess
//! writes a request to `~/.fleet/guard/<uuid>.json` and polls for a response
//! file at `~/.fleet/guard/<uuid>.response.json`.  The Fleet desktop app
//! watches this directory, shows a dialog to the user, and writes the response.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::audit::{self, AuditRiskLevel};

// ── Types ────────────────────────────────────────────────────────────────────

/// Decision the user makes in the guard dialog.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GuardDecision {
    Allow,
    Block,
}

/// Written by `fleet guard` → read by Fleet desktop app.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GuardRequest {
    pub id: String,
    pub session_id: String,
    pub workspace_name: String,
    /// AI-generated session title (distinct from workspace_name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    pub tool_name: String,
    pub command: String,
    pub command_summary: String,
    pub risk_tags: Vec<String>,
    pub timestamp: String,
}

/// Written by Fleet desktop app → read by `fleet guard`.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct GuardResponse {
    pub id: String,
    pub decision: GuardDecision,
}

// ── Paths ────────────────────────────────────────────────────────────────────

pub fn guard_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("guard"))
}

fn request_path(id: &str) -> Option<PathBuf> {
    guard_dir().map(|d| d.join(format!("{id}.json")))
}

fn response_path(id: &str) -> Option<PathBuf> {
    guard_dir().map(|d| d.join(format!("{id}.response.json")))
}

// ── File-based IPC ───────────────────────────────────────────────────────────

/// Write a guard request.  Called by `fleet guard` CLI.
pub fn write_request(req: &GuardRequest) -> Result<(), String> {
    let dir = guard_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create guard dir: {e}"))?;
    let path = request_path(&req.id).unwrap();
    let json = serde_json::to_string_pretty(req).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write request: {e}"))
}

/// Poll for a guard response.  Called by `fleet guard` CLI.
/// Returns `None` on timeout.
pub fn poll_response(id: &str, timeout: Duration) -> Option<GuardResponse> {
    let path = response_path(id)?;
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(200);

    loop {
        if start.elapsed() > timeout {
            return None;
        }
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(resp) = serde_json::from_str::<GuardResponse>(&content) {
                    return Some(resp);
                }
            }
        }
        std::thread::sleep(poll_interval);
    }
}

/// Non-blocking read of a guard response, if one exists yet.
pub fn try_read_response(id: &str) -> Option<GuardResponse> {
    let path = response_path(id)?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<GuardResponse>(&content).ok()
}

/// Write a guard response.  Called by the desktop app.
pub fn write_response(resp: &GuardResponse) -> Result<(), String> {
    let path = response_path(&resp.id).ok_or("cannot determine home dir")?;
    let json = serde_json::to_string(resp).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write response: {e}"))
}

/// Clean up request + response files.
pub fn cleanup(id: &str) {
    if let Some(p) = request_path(id) {
        let _ = fs::remove_file(p);
    }
    if let Some(p) = response_path(id) {
        let _ = fs::remove_file(p);
    }
}

/// Read a pending request file.
pub fn read_request(id: &str) -> Option<GuardRequest> {
    let path = request_path(id)?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// List all pending request IDs in the guard directory. Soft form —
/// returns an empty vec on any failure. See [`list_pending_requests_checked`]
/// for the variant that distinguishes "no requests" from "couldn't read".
pub fn list_pending_requests() -> Vec<String> {
    list_pending_requests_checked().unwrap_or_default()
}

/// Strict form — `Ok(vec![])` for "directory missing / no requests" but
/// `Err` for actual I/O errors. The directory watcher uses this so a
/// transient `read_dir` error doesn't get treated as "all requests vanished"
/// and dismiss every active panel.
pub fn list_pending_requests_checked() -> std::io::Result<Vec<String>> {
    let Some(dir) = guard_dir() else {
        return Ok(Vec::new());
    };
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    Ok(entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".json") && !name.contains(".response.") {
                Some(name.trim_end_matches(".json").to_string())
            } else {
                None
            }
        })
        .collect())
}

// ── Guard logic (used by `fleet guard` CLI) ─────────────────────────────────

/// Parsed hook input from Claude Code PreToolUse.
#[derive(Deserialize, Debug)]
pub struct HookInput {
    pub session_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_input: Option<serde_json::Value>,
}

/// Result of classifying the hook input.
pub enum GuardClassification {
    /// Not a Bash command, or not Critical — allow silently.
    Allow,
    /// Critical risk — needs user confirmation.
    NeedsConfirmation {
        command: String,
        risk_tags: Vec<String>,
    },
}

/// Classify a hook input.  Returns whether this needs user confirmation.
pub fn classify_hook_input(input: &HookInput) -> GuardClassification {
    let tool_name = input.tool_name.as_deref().unwrap_or("");
    if tool_name != "Bash" {
        return GuardClassification::Allow;
    }

    let command = input
        .tool_input
        .as_ref()
        .and_then(|v| v.get("command"))
        .and_then(|c| c.as_str())
        .unwrap_or("");

    if command.is_empty() {
        return GuardClassification::Allow;
    }

    match audit::classify_bash_command_pub(command) {
        Some((AuditRiskLevel::Critical, tags)) => GuardClassification::NeedsConfirmation {
            command: command.to_string(),
            risk_tags: tags,
        },
        _ => GuardClassification::Allow,
    }
}

/// Generate a new unique guard request ID.
pub fn new_request_id() -> String {
    Uuid::new_v4().to_string()
}

/// Truncate a command for display.
pub fn truncate_command(cmd: &str, max: usize) -> String {
    if cmd.len() <= max {
        cmd.to_string()
    } else {
        format!("{}…", &cmd[..max])
    }
}

// ── LLM analysis prompt ─────────────────────────────────────────────────────

/// Build a prompt for the LLM to analyze a guarded command.
pub fn build_analysis_prompt(
    command: &str,
    risk_tags: &[String],
    context_message: &str,
    lang: &str,
) -> String {
    let lang_instruction = match lang {
        "zh" => "请用中文回答。",
        _ => "Answer in English.",
    };

    format!(
        r#"You are a security analyst reviewing a command about to be executed by an AI coding agent.

Context (the agent's last message before this tool call):
{context_message}

Command to be executed:
```
{command}
```

Risk tags: {tags}

{lang_instruction}
In 2-3 concise sentences:
1. What this command is doing
2. What the specific security risk is
3. Whether this seems intentional given the context (false positive?)"#,
        context_message = if context_message.is_empty() {
            "(no context available)"
        } else {
            context_message
        },
        command = command,
        tags = risk_tags.join(", "),
        lang_instruction = lang_instruction,
    )
}
