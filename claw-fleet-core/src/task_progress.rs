//! Per-session TASKS.md focus — "which plan / which P is THIS session working
//! on right now", captured by the `fleet plan` subcommands.
//!
//! Why a Fleet-maintained side-channel instead of markers in TASKS.md itself:
//! the "current" plan/task is *per-session* dynamic state. Multiple sessions
//! routinely share one workspace + one TASKS.md (simple mode, no worktree), so
//! an in-file in-progress marker would let sessions clobber each other and
//! couldn't be attributed. The agent also can't read the wall clock to stamp a
//! timestamp. The `fleet plan` subcommands run inside Fleet (they know
//! `FLEET_SESSION_ID` and the system clock), so they record focus here, keyed
//! by session id — TASKS.md stays the durable, shared, checkbox-only plan.
//!
//! File layout: `~/.fleet/task-progress/<session_id>.json`.

use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgressRecord {
    /// Workspace the plan lives in — guards against a stale record from a
    /// different workspace bleeding into this session's card.
    pub workspace_path: String,
    /// Sentinel id of the plan this session is focused on.
    pub plan_id: String,
    /// First pending P text at the time of the update (e.g. `**P3** — …`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub current_task: Option<String>,
    /// Epoch milliseconds of the update — newest wins when reconciling.
    pub updated: u64,
}

fn progress_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("task-progress"))
}

fn record_path(session_id: &str) -> Option<PathBuf> {
    progress_dir().map(|d| d.join(format!("{session_id}.json")))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Record this session's current plan focus. Idempotent — re-recording
/// refreshes the timestamp. `current_task` is the focused plan's first pending
/// P (None when the plan has none left).
pub fn set_current(
    session_id: &str,
    workspace_path: &str,
    plan_id: &str,
    current_task: Option<String>,
) -> Result<(), String> {
    let dir = progress_dir().ok_or("cannot determine home dir")?;
    fs::create_dir_all(&dir).map_err(|e| format!("create task-progress dir: {e}"))?;
    let path = record_path(session_id).unwrap();
    let rec = TaskProgressRecord {
        workspace_path: workspace_path.to_string(),
        plan_id: plan_id.to_string(),
        current_task,
        updated: now_ms(),
    };
    let json = serde_json::to_string(&rec).map_err(|e| format!("serialize: {e}"))?;
    fs::write(&path, json).map_err(|e| format!("write task-progress: {e}"))
}

/// Read a session's current plan focus, if any.
pub fn read(session_id: &str) -> Option<TaskProgressRecord> {
    let path = record_path(session_id)?;
    let s = fs::read_to_string(&path).ok()?;
    serde_json::from_str(&s).ok()
}

/// Clear a session's focus record. Idempotent — missing file is OK.
pub fn clear(session_id: &str) {
    if let Some(p) = record_path(session_id) {
        let _ = fs::remove_file(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_roundtrips_through_serde() {
        let rec = TaskProgressRecord {
            workspace_path: "/ws".to_string(),
            plan_id: "auth".to_string(),
            current_task: Some("**P2** — swap".to_string()),
            updated: 123,
        };
        let json = serde_json::to_string(&rec).unwrap();
        // camelCase on the wire
        assert!(json.contains("\"workspacePath\""));
        assert!(json.contains("\"planId\""));
        assert!(json.contains("\"currentTask\""));
        let back: TaskProgressRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn current_task_omitted_when_none() {
        let rec = TaskProgressRecord {
            workspace_path: "/ws".to_string(),
            plan_id: "x".to_string(),
            current_task: None,
            updated: 1,
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(!json.contains("currentTask"));
        let back: TaskProgressRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back.current_task, None);
    }
}
