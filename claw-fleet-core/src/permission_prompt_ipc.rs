//! IPC protocol for the `fleet__permission_prompt` MCP tool — the bridge that
//! turns Claude Code's *native permission prompts* into Fleet Decision Cards
//! for headless (`claude -p`) sessions.
//!
//! Claude Code's hidden `--permission-prompt-tool <mcp tool>` flag routes
//! every would-be-interactive permission request (a tool call that is neither
//! allowed nor denied by permission rules) to the named MCP tool instead of
//! silently denying it. The tool receives `{tool_name, input, tool_use_id}`
//! and must answer with a JSON text payload of either
//! `{"behavior":"allow","updatedInput":{...}}` or
//! `{"behavior":"deny","message":"..."}`. (Contract verified empirically
//! against CLI 2.1.181: the flag is absent from `--help` but parsed, the
//! call arrives as a normal `tools/call`, and both behaviors round-trip.)
//!
//! Wire format: file-based IPC at `~/.fleet/permission-prompt/<id>.json`
//! (request) and `<id>.response.json` (response), mirroring the
//! `elicitation` / `guard` / `fleet-ask` Decision Card channels so the
//! desktop / `fleet serve` watchers can reuse the same polling shape.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

/// Written by the `fleet mcp` server → read by Fleet desktop / `fleet serve`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionPromptRequest {
    pub id: String,
    /// Originating Claude Code session id (from `CLAUDE_CODE_SESSION_ID`,
    /// same resolution as `FleetAskRequest::session_id`).
    #[serde(default)]
    pub session_id: String,
    #[serde(default)]
    pub workspace_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    #[serde(default)]
    pub timestamp: String,
    /// The tool Claude Code wants to run (e.g. `Write`, `Bash`,
    /// `NotebookEdit`, an MCP tool name).
    pub tool_name: String,
    /// The tool's full input payload, displayed on the card so the user can
    /// judge the action. Passed back verbatim as `updatedInput` on allow.
    #[serde(default)]
    pub tool_input: serde_json::Value,
    /// Claude Code's tool_use id, if provided; carried for log correlation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_use_id: Option<String>,
}

/// Decision the user makes on the permission-prompt Decision Card.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionPromptDecision {
    Allow,
    Deny,
}

/// Written by Fleet desktop / `fleet serve` → read by the `fleet mcp` server.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionPromptResponse {
    pub id: String,
    pub decision: PermissionPromptDecision,
    /// Optional user-typed reason (deny path); forwarded to the agent as the
    /// deny `message` so it knows why the action was refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

// ── File-based IPC (mirror of `mcp_ipc`) ─────────────────────────────────────

fn permission_prompt_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("permission-prompt"))
}

fn request_path(id: &str) -> Option<PathBuf> {
    permission_prompt_dir().map(|d| d.join(format!("{id}.json")))
}

fn response_path(id: &str) -> Option<PathBuf> {
    permission_prompt_dir().map(|d| d.join(format!("{id}.response.json")))
}

/// Write a permission-prompt request file. Called by the MCP server when it
/// handles a `tools/call` for `fleet__permission_prompt`.
pub fn write_request(req: &PermissionPromptRequest) -> Result<(), String> {
    let dir = permission_prompt_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create permission-prompt dir: {e}"))?;
    let path = request_path(&req.id).unwrap();
    let json = serde_json::to_string_pretty(req).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write permission-prompt request: {e}"))
}

/// Read a pending request file. Called by the desktop watcher when it
/// notices a new id in the directory listing.
pub fn read_request(id: &str) -> Option<PermissionPromptRequest> {
    let path = request_path(id)?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Non-blocking response read.
pub fn try_read_response(id: &str) -> Option<PermissionPromptResponse> {
    let path = response_path(id)?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<PermissionPromptResponse>(&content).ok()
}

/// Blocking poll for a response. Returns `None` on timeout.
pub fn poll_response(id: &str, timeout: Duration) -> Option<PermissionPromptResponse> {
    let start = std::time::Instant::now();
    let interval = Duration::from_millis(200);
    loop {
        if let Some(r) = try_read_response(id) {
            return Some(r);
        }
        if start.elapsed() > timeout {
            return None;
        }
        std::thread::sleep(interval);
    }
}

/// Write a response file. Called by the desktop / fleet serve after the
/// user resolves the Decision Card.
pub fn write_response(resp: &PermissionPromptResponse) -> Result<(), String> {
    let path = response_path(&resp.id).ok_or("cannot determine home dir")?;
    let json = serde_json::to_string(resp).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write permission-prompt response: {e}"))
}

/// Remove the request + response pair. Called by the MCP server once it has
/// consumed the response.
pub fn cleanup(id: &str) {
    if let Some(p) = request_path(id) {
        let _ = fs::remove_file(p);
    }
    if let Some(p) = response_path(id) {
        let _ = fs::remove_file(p);
    }
}

/// Soft list of pending request ids; returns empty on any failure.
pub fn list_pending_requests() -> Vec<String> {
    list_pending_requests_checked().unwrap_or_default()
}

/// Strict list — distinguishes "no requests / missing dir" (`Ok(vec![])`)
/// from "I/O error reading the directory" (`Err`), same contract as
/// `mcp_ipc::list_pending_requests_checked`.
pub fn list_pending_requests_checked() -> std::io::Result<Vec<String>> {
    let Some(dir) = permission_prompt_dir() else {
        return Ok(Vec::new());
    };
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };
    let mut request_ids: Vec<String> = Vec::new();
    let mut response_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name().to_string_lossy().to_string();
        if let Some(id) = name.strip_suffix(".response.json") {
            response_ids.insert(id.to_string());
        } else if let Some(id) = name.strip_suffix(".json") {
            request_ids.push(id.to_string());
        }
    }
    Ok(request_ids
        .into_iter()
        .filter(|id| !response_ids.contains(id))
        .collect())
}

/// JSONSchema for `fleet__permission_prompt` arguments, embedded verbatim in
/// the MCP `tools/list` response. Field names are what Claude Code actually
/// sends (snake_case, observed on CLI 2.1.181): `tool_name`, `input`,
/// `tool_use_id`.
pub fn permission_prompt_input_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "tool_name": { "type": "string" },
            "input": { "type": "object" },
            "tool_use_id": { "type": "string" }
        },
        "required": ["tool_name", "input"]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_roundtrip_allow_and_deny() {
        let allow = PermissionPromptResponse {
            id: "abc".into(),
            decision: PermissionPromptDecision::Allow,
            reason: None,
        };
        let json = serde_json::to_string(&allow).unwrap();
        assert!(json.contains("\"allow\""), "snake_case wire form: {json}");
        let back: PermissionPromptResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(back.decision, PermissionPromptDecision::Allow);

        let deny: PermissionPromptResponse = serde_json::from_str(
            r#"{"id":"x","decision":"deny","reason":"nope"}"#,
        )
        .unwrap();
        assert_eq!(deny.decision, PermissionPromptDecision::Deny);
        assert_eq!(deny.reason.as_deref(), Some("nope"));
    }

    #[test]
    fn request_deserializes_without_optional_fields() {
        // The MCP server builds requests itself, but the desktop / fleet
        // serve must tolerate a minimal payload (forward compatibility).
        let req: PermissionPromptRequest = serde_json::from_str(
            r#"{"id":"r1","toolName":"Write","toolInput":{"file_path":"/tmp/x"}}"#,
        )
        .unwrap();
        assert_eq!(req.tool_name, "Write");
        assert!(req.tool_use_id.is_none());
        assert_eq!(req.tool_input["file_path"], "/tmp/x");
    }
}
