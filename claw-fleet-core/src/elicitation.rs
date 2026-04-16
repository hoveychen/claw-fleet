//! Elicitation — intercepts `AskUserQuestion` tool calls from Claude Code
//! via a synchronous `PreToolUse` hook, routing the questions to the Fleet
//! desktop app for the user to answer.
//!
//! Uses the same file-based IPC pattern as `guard`: requests go to
//! `~/.fleet/elicitation/<uuid>.json`, responses to
//! `~/.fleet/elicitation/<uuid>.response.json`.  The `fleet elicitation` CLI
//! outputs a `hookSpecificOutput` with `updatedInput` containing the user's
//! answers.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

// ── Types ────────────────────────────────────────────────────────────────────

/// A single question option from AskUserQuestion.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationOption {
    pub label: String,
    pub description: String,
}

/// A single question from AskUserQuestion.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationQuestion {
    pub question: String,
    pub header: String,
    pub options: Vec<ElicitationOption>,
    #[serde(default)]
    pub multi_select: bool,
}

/// Written by `fleet elicitation` → read by Fleet desktop app.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationRequest {
    pub id: String,
    pub session_id: String,
    pub workspace_name: String,
    pub questions: Vec<ElicitationQuestion>,
    pub timestamp: String,
}

/// Written by Fleet desktop app → read by `fleet elicitation`.
/// `answers` maps question text → selected option label(s).
/// If the user declines, `declined` is true and `answers` is empty.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ElicitationResponse {
    pub id: String,
    #[serde(default)]
    pub declined: bool,
    pub answers: HashMap<String, String>,
}

// ── Paths ────────────────────────────────────────────────────────────────────

fn elicitation_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("elicitation"))
}

fn request_path(id: &str) -> Option<PathBuf> {
    elicitation_dir().map(|d| d.join(format!("{id}.json")))
}

fn response_path(id: &str) -> Option<PathBuf> {
    elicitation_dir().map(|d| d.join(format!("{id}.response.json")))
}

// ── File-based IPC ───────────────────────────────────────────────────────────

/// Write an elicitation request.  Called by `fleet elicitation` CLI.
pub fn write_request(req: &ElicitationRequest) -> Result<(), String> {
    let dir = elicitation_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create elicitation dir: {e}"))?;
    let path = request_path(&req.id).unwrap();
    let json = serde_json::to_string_pretty(req).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write elicitation request: {e}"))
}

/// Poll for an elicitation response.  Called by `fleet elicitation` CLI.
/// Returns `None` on timeout.
pub fn poll_response(id: &str, timeout: Duration) -> Option<ElicitationResponse> {
    let path = response_path(id)?;
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(200);

    loop {
        if start.elapsed() > timeout {
            return None;
        }
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(resp) = serde_json::from_str::<ElicitationResponse>(&content) {
                    return Some(resp);
                }
            }
        }
        std::thread::sleep(poll_interval);
    }
}

/// Write an elicitation response.  Called by the desktop app.
pub fn write_response(resp: &ElicitationResponse) -> Result<(), String> {
    let path = response_path(&resp.id).ok_or("cannot determine home dir")?;
    let json = serde_json::to_string(resp).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write elicitation response: {e}"))
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
pub fn read_request(id: &str) -> Option<ElicitationRequest> {
    let path = request_path(id)?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// List all pending request IDs in the elicitation directory.
pub fn list_pending_requests() -> Vec<String> {
    let Some(dir) = elicitation_dir() else {
        return Vec::new();
    };
    let Ok(entries) = fs::read_dir(&dir) else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            if name.ends_with(".json") && !name.contains(".response.") {
                Some(name.trim_end_matches(".json").to_string())
            } else {
                None
            }
        })
        .collect()
}
