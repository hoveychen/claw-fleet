//! Plan approval — intercepts `ExitPlanMode` tool calls from Claude Code
//! via a synchronous `PreToolUse` hook, routing the plan content to the Fleet
//! desktop app for the user to approve, reject, or edit.
//!
//! Uses the same file-based IPC pattern as `elicitation`: requests go to
//! `~/.fleet/plan-approval/<uuid>.json`, responses to
//! `~/.fleet/plan-approval/<uuid>.response.json`.  The `fleet plan-approval`
//! CLI outputs a `hookSpecificOutput` with `permissionDecision` (allow/deny)
//! and — on approve-with-edits — `updatedInput.plan` carrying the edited plan.

use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

// ── Types ────────────────────────────────────────────────────────────────────

/// Written by `fleet plan-approval` → read by Fleet desktop app.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlanApprovalRequest {
    pub id: String,
    pub session_id: String,
    pub workspace_name: String,
    /// AI-generated session title (distinct from workspace_name).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_title: Option<String>,
    /// The plan markdown content (already extracted from tool_input.plan).
    pub plan_content: String,
    /// Absolute path where the plan is persisted (from tool_input.planFilePath).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_file_path: Option<String>,
    pub timestamp: String,
}

/// Written by Fleet desktop app → read by `fleet plan-approval`.
///
/// `decision` is `"approve"` or `"reject"`.
/// * approve: optionally include `edited_plan` to replace the plan content.
/// * reject: optionally include `feedback` that gets surfaced to the model.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlanApprovalResponse {
    pub id: String,
    pub decision: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited_plan: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub feedback: Option<String>,
}

// ── Paths ────────────────────────────────────────────────────────────────────

fn plan_approval_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("plan-approval"))
}

fn request_path(id: &str) -> Option<PathBuf> {
    plan_approval_dir().map(|d| d.join(format!("{id}.json")))
}

fn response_path(id: &str) -> Option<PathBuf> {
    plan_approval_dir().map(|d| d.join(format!("{id}.response.json")))
}

// ── File-based IPC ───────────────────────────────────────────────────────────

/// Write a plan approval request.  Called by `fleet plan-approval` CLI.
pub fn write_request(req: &PlanApprovalRequest) -> Result<(), String> {
    let dir = plan_approval_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create plan-approval dir: {e}"))?;
    let path = request_path(&req.id).unwrap();
    let json = serde_json::to_string_pretty(req).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write plan-approval request: {e}"))
}

/// Poll for a plan approval response.  Called by `fleet plan-approval` CLI.
/// Returns `None` on timeout.
pub fn poll_response(id: &str, timeout: Duration) -> Option<PlanApprovalResponse> {
    let path = response_path(id)?;
    let start = std::time::Instant::now();
    let poll_interval = Duration::from_millis(200);

    loop {
        if start.elapsed() > timeout {
            return None;
        }
        if path.exists() {
            if let Ok(content) = fs::read_to_string(&path) {
                if let Ok(resp) = serde_json::from_str::<PlanApprovalResponse>(&content) {
                    return Some(resp);
                }
            }
        }
        std::thread::sleep(poll_interval);
    }
}

/// Non-blocking read of a plan approval response, if one exists yet.
pub fn try_read_response(id: &str) -> Option<PlanApprovalResponse> {
    let path = response_path(id)?;
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<PlanApprovalResponse>(&content).ok()
}

/// Write a plan approval response.  Called by the desktop app.
pub fn write_response(resp: &PlanApprovalResponse) -> Result<(), String> {
    let path = response_path(&resp.id).ok_or("cannot determine home dir")?;
    let json = serde_json::to_string(resp).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write plan-approval response: {e}"))
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
pub fn read_request(id: &str) -> Option<PlanApprovalRequest> {
    let path = request_path(id)?;
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// List all pending request IDs in the plan-approval directory. Soft form
/// — returns an empty vec on any failure. See [`list_pending_requests_checked`]
/// for the variant that distinguishes "no requests" from "couldn't read".
pub fn list_pending_requests() -> Vec<String> {
    list_pending_requests_checked().unwrap_or_default()
}

/// Strict form — `Ok(vec![])` for "directory missing / no requests" but
/// `Err` for actual I/O errors. The directory watcher uses this so a
/// transient `read_dir` error doesn't get treated as "all requests vanished"
/// and dismiss every active panel.
pub fn list_pending_requests_checked() -> std::io::Result<Vec<String>> {
    let Some(dir) = plan_approval_dir() else {
        return Ok(Vec::new());
    };
    list_pending_in_dir(&dir)
}

fn list_pending_in_dir(dir: &std::path::Path) -> std::io::Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e),
    };

    // Two-pass: drop any request whose partner `<id>.response.json` already
    // exists.  See `elicitation::list_pending_in_dir` for the full rationale.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_request_serde() {
        let req = PlanApprovalRequest {
            id: "abc".into(),
            session_id: "s1".into(),
            workspace_name: "claude-fleet".into(),
            ai_title: Some("Intercept ExitPlanMode".into()),
            plan_content: "# plan\n- step".into(),
            plan_file_path: Some("/tmp/plan.md".into()),
            timestamp: "2026-04-20T00:00:00Z".into(),
        };
        let s = serde_json::to_string(&req).unwrap();
        assert!(s.contains("\"planContent\""));
        assert!(s.contains("\"planFilePath\""));
        let back: PlanApprovalRequest = serde_json::from_str(&s).unwrap();
        assert_eq!(back.id, "abc");
        assert_eq!(back.plan_content, "# plan\n- step");
    }

    #[test]
    fn response_omits_optional_fields_when_none() {
        let resp = PlanApprovalResponse {
            id: "abc".into(),
            decision: "approve".into(),
            edited_plan: None,
            feedback: None,
        };
        let s = serde_json::to_string(&resp).unwrap();
        assert!(!s.contains("editedPlan"));
        assert!(!s.contains("feedback"));
    }

    #[test]
    fn response_with_edited_plan_roundtrips() {
        let resp = PlanApprovalResponse {
            id: "abc".into(),
            decision: "approve".into(),
            edited_plan: Some("edited".into()),
            feedback: None,
        };
        let s = serde_json::to_string(&resp).unwrap();
        let back: PlanApprovalResponse = serde_json::from_str(&s).unwrap();
        assert_eq!(back.edited_plan.as_deref(), Some("edited"));
    }
}
