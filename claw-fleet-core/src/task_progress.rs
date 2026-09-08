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

pub(crate) fn progress_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("task-progress"))
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
    set_current_in(&dir, session_id, workspace_path, plan_id, current_task)
}

/// Read a session's current plan focus, if any.
pub fn read(session_id: &str) -> Option<TaskProgressRecord> {
    read_in(&progress_dir()?, session_id)
}

/// Clear a session's focus record. Idempotent — missing file is OK.
pub fn clear(session_id: &str) {
    if let Some(d) = progress_dir() {
        clear_in(&d, session_id);
    }
}

// ── Directory-injecting variants ──────────────────────────────────────────────
//
// The public wrappers above resolve the record directory from `FLEET_HOME` /
// the passwd entry, which is process-global state that other tests mutate. Tests
// take these `_in` forms with a tempdir instead, so they neither race that env
// var nor touch the developer's real `~/.fleet`. Same split as `handoff.rs`'s
// `register_in`.

pub(crate) fn set_current_in(
    dir: &std::path::Path,
    session_id: &str,
    workspace_path: &str,
    plan_id: &str,
    current_task: Option<String>,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create task-progress dir: {e}"))?;
    let rec = TaskProgressRecord {
        workspace_path: workspace_path.to_string(),
        plan_id: plan_id.to_string(),
        current_task,
        updated: now_ms(),
    };
    let json = serde_json::to_string(&rec).map_err(|e| format!("serialize: {e}"))?;
    fs::write(dir.join(format!("{session_id}.json")), json)
        .map_err(|e| format!("write task-progress: {e}"))
}

pub(crate) fn read_in(dir: &std::path::Path, session_id: &str) -> Option<TaskProgressRecord> {
    let s = fs::read_to_string(dir.join(format!("{session_id}.json"))).ok()?;
    serde_json::from_str(&s).ok()
}

pub(crate) fn clear_in(dir: &std::path::Path, session_id: &str) {
    let _ = fs::remove_file(dir.join(format!("{session_id}.json")));
}

/// Newest focus timestamp per plan id, over every session's record.
///
/// "Which plans is somebody actually on" — the injection uses it to put those
/// first in the list an unattributed session sees. No age cutoff: a record only
/// exists because a session claimed that plan, and "claimed three days ago"
/// still outranks "nobody ever touched it" as a reason to show it. Records are
/// keyed by session, so several sessions on one plan collapse to the newest.
/// Ids are matched across every workspace — the same id in two repos can only
/// nudge one list's ordering, which is not worth a path-canonicalising filter.
pub fn latest_focus_by_plan() -> std::collections::HashMap<String, u64> {
    progress_dir()
        .map(|d| latest_focus_by_plan_in(&d))
        .unwrap_or_default()
}

pub(crate) fn latest_focus_by_plan_in(
    dir: &std::path::Path,
) -> std::collections::HashMap<String, u64> {
    let mut out: std::collections::HashMap<String, u64> = std::collections::HashMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return out;
    };
    for e in entries.flatten() {
        let Ok(s) = fs::read_to_string(e.path()) else {
            continue;
        };
        let Ok(rec) = serde_json::from_str::<TaskProgressRecord>(&s) else {
            continue;
        };
        let slot = out.entry(rec.plan_id).or_insert(0);
        *slot = (*slot).max(rec.updated);
    }
    out
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
