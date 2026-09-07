//! Per-session **task terminal state** — "did this task end in success or was it
//! given up on?".
//!
//! This is the v3 decision-card axis. Every `fleet__ask` card carries one
//! always-present first-class button that ends the task: rendered as
//! **结束任务 / Finish task** when the agent flagged `taskComplete`, and as
//! **放弃任务 / Abandon task** when it did not. Clicking it resolves the card
//! *and* stamps the session with the outcome recorded here. Before v3 the agent
//! had to hand-roll a "任务结束" option into `options`, which produced no
//! machine-readable terminal state at all — the whole point of this module is
//! that the retrospective (`task_review`) now knows which traces ended well.
//!
//! Three axes, deliberately kept apart:
//!   * [`crate::session::SessionStatus`] — what the agent is doing *right now*
//!     (auto-computed, active/idle).
//!   * [`crate::session_mark::SessionMark`] — "have *I*, the human, reviewed
//!     this?" (pending/done).
//!   * this module — "how did the task *end*?" (completed/abandoned).
//!
//! Collapsing the last two into one enum was considered and rejected: a task can
//! be abandoned yet still need review, and marking a session reviewed says
//! nothing about whether the work succeeded. Setting an outcome does, however,
//! also stamp `SessionMark::Done` at the call site — reaching a terminal state
//! implies the human is finished with it, and that keeps the existing
//! review-queue filters honest.
//!
//! Same side-channel pattern as `session_mark` / `task_progress`: a
//! Fleet-maintained file keyed by session id, folded back into `SessionInfo` at
//! scan time (the non-cached enrich stage, never the cached deep parse — the
//! outcome changes while the jsonl does not).
//!
//! File layout: `~/.fleet/task-outcome/<session_id>.json`.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// How a task ended. Absence of a record means "not terminated" — still open.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "lowercase")]
pub enum TaskOutcome {
    /// The user pressed 结束任务 — the work is done and it succeeded.
    Completed,
    /// The user pressed 放弃任务 — the task was given up on, unfinished.
    Abandoned,
}

impl TaskOutcome {
    /// Whether this outcome counts as a success for retrospective grouping.
    pub fn is_success(self) -> bool {
        matches!(self, TaskOutcome::Completed)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct TaskOutcomeRecord {
    pub outcome: TaskOutcome,
    /// Workspace the session lived in — not load-bearing (session id is the
    /// unique key) but kept for parity with `session_mark` and so the
    /// retrospective can group by project without a second lookup.
    pub workspace_path: String,
    /// The decision card whose terminal button produced this outcome. Empty when
    /// the outcome was set some other way (e.g. a manual override from the
    /// session list).
    #[serde(default)]
    pub card_id: String,
    /// What the agent *claimed* when it raised that card (`taskComplete`).
    /// Keeping it alongside the user's verdict is the whole point: a card raised
    /// with `taskComplete: true` that the user still abandoned is the single
    /// strongest "the agent thought it was done and it wasn't" signal the
    /// retrospective has.
    #[serde(default)]
    pub agent_claimed_complete: bool,
    /// Epoch milliseconds of the update.
    pub updated: u64,
}

/// HTTP request body for setting an outcome over the `fleet serve` boundary
/// (`RemoteBackend` → `/task_outcome`). `outcome: None` clears.
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SetTaskOutcomeRequest {
    pub session_id: String,
    pub workspace_path: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub outcome: Option<TaskOutcome>,
}

pub(crate) fn outcome_dir() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("task-outcome"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Set (or clear) a session's task outcome. `Some(..)` writes/overwrites,
/// `None` clears (removing the file). Idempotent either way.
pub fn set_outcome(
    session_id: &str,
    workspace_path: &str,
    outcome: Option<TaskOutcome>,
    card_id: &str,
    agent_claimed_complete: bool,
) -> Result<(), String> {
    let dir = outcome_dir().ok_or("cannot determine home dir")?;
    set_outcome_in(
        &dir,
        session_id,
        workspace_path,
        outcome,
        card_id,
        agent_claimed_complete,
    )
}

/// Read a session's outcome record, if any.
pub fn read(session_id: &str) -> Option<TaskOutcomeRecord> {
    read_in(&outcome_dir()?, session_id)
}

/// Clear a terminal outcome when the human resumes the session. A task that is
/// running again is no longer finished — same reasoning as
/// [`crate::session_mark::clear_done_on_resume`], and called from the same
/// user-initiated resume entry points (never the automatic ones).
pub fn clear_on_resume(session_id: &str) {
    let Some(dir) = outcome_dir() else { return };
    clear_on_resume_in(&dir, session_id);
}

/// Stamp each session's `task_outcome` from the on-disk records. One directory
/// scan into an index, then a map over the sessions — mirrors
/// `session_mark::enrich_sessions`.
pub fn enrich_sessions(sessions: &mut [crate::session::SessionInfo]) {
    let Some(dir) = outcome_dir() else { return };
    enrich_sessions_in(&dir, sessions);
}

/// The index is the whole truth: a session missing from it has no outcome, so an
/// empty index clears every stamp rather than being a no-op (re-stamping an
/// already-enriched cached list must be able to drop the last record).
pub(crate) fn enrich_sessions_in(
    dir: &std::path::Path,
    sessions: &mut [crate::session::SessionInfo],
) {
    let idx = outcome_index(dir);
    for s in sessions.iter_mut() {
        s.task_outcome = idx.get(&s.id).map(|r| r.outcome);
    }
}

// ── Directory-injecting variants ─────────────────────────────────────────────
//
// Same split as `session_mark` / `task_progress`: the public wrappers resolve
// the directory from process-global state, tests take a tempdir via these `_in`
// forms so they neither race `$FLEET_HOME` nor touch the real `~/.fleet`.

pub(crate) fn set_outcome_in(
    dir: &std::path::Path,
    session_id: &str,
    workspace_path: &str,
    outcome: Option<TaskOutcome>,
    card_id: &str,
    agent_claimed_complete: bool,
) -> Result<(), String> {
    let path = dir.join(format!("{session_id}.json"));
    match outcome {
        None => {
            let _ = fs::remove_file(&path);
            Ok(())
        }
        Some(outcome) => {
            fs::create_dir_all(dir).map_err(|e| format!("create task-outcome dir: {e}"))?;
            let rec = TaskOutcomeRecord {
                outcome,
                workspace_path: workspace_path.to_string(),
                card_id: card_id.to_string(),
                agent_claimed_complete,
                updated: now_ms(),
            };
            let json = serde_json::to_string(&rec).map_err(|e| format!("serialize: {e}"))?;
            fs::write(&path, json).map_err(|e| format!("write task-outcome: {e}"))
        }
    }
}

pub(crate) fn read_in(dir: &std::path::Path, session_id: &str) -> Option<TaskOutcomeRecord> {
    let s = fs::read_to_string(dir.join(format!("{session_id}.json"))).ok()?;
    serde_json::from_str(&s).ok()
}

pub(crate) fn clear_on_resume_in(dir: &std::path::Path, session_id: &str) {
    if read_in(dir, session_id).is_some() {
        let _ = fs::remove_file(dir.join(format!("{session_id}.json")));
    }
}

/// Read the whole `task-outcome/` directory into `session_id → record`.
/// Malformed or non-`.json` entries are skipped.
pub fn outcome_index(dir: &std::path::Path) -> HashMap<String, TaskOutcomeRecord> {
    let mut idx = HashMap::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return idx;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let Some(id) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if let Some(rec) = read_in(dir, id) {
            idx.insert(id.to_string(), rec);
        }
    }
    idx
}

/// Every recorded outcome, keyed by session id. Used by the retrospective to
/// find the day's terminated tasks without re-walking every transcript.
pub fn all_outcomes() -> HashMap<String, TaskOutcomeRecord> {
    outcome_dir().map(|d| outcome_index(&d)).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmpdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "fleet-task-outcome-{tag}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn record_roundtrips_camelcase() {
        let rec = TaskOutcomeRecord {
            outcome: TaskOutcome::Abandoned,
            workspace_path: "/ws".to_string(),
            card_id: "card-1".to_string(),
            agent_claimed_complete: true,
            updated: 123,
        };
        let json = serde_json::to_string(&rec).unwrap();
        assert!(json.contains("\"workspacePath\""));
        assert!(json.contains("\"agentClaimedComplete\":true"));
        assert!(json.contains("\"outcome\":\"abandoned\""));
        let back: TaskOutcomeRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(back, rec);
    }

    #[test]
    fn set_read_clear_roundtrip() {
        let dir = tmpdir("crud");
        assert!(read_in(&dir, "s1").is_none());

        set_outcome_in(&dir, "s1", "/ws", Some(TaskOutcome::Completed), "c1", true).unwrap();
        let rec = read_in(&dir, "s1").unwrap();
        assert_eq!(rec.outcome, TaskOutcome::Completed);
        assert_eq!(rec.card_id, "c1");
        assert!(rec.agent_claimed_complete);

        // Overwrite with the opposite verdict.
        set_outcome_in(&dir, "s1", "/ws", Some(TaskOutcome::Abandoned), "c2", true).unwrap();
        assert_eq!(read_in(&dir, "s1").unwrap().outcome, TaskOutcome::Abandoned);

        set_outcome_in(&dir, "s1", "/ws", None, "", false).unwrap();
        assert!(read_in(&dir, "s1").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn resume_clears_any_outcome() {
        let dir = tmpdir("resume");
        set_outcome_in(&dir, "s1", "/ws", Some(TaskOutcome::Completed), "c1", true).unwrap();
        clear_on_resume_in(&dir, "s1");
        assert!(
            read_in(&dir, "s1").is_none(),
            "a resumed session is no longer terminal"
        );
        // Abandoned clears too — resuming an abandoned task un-abandons it.
        set_outcome_in(&dir, "s2", "/ws", Some(TaskOutcome::Abandoned), "c2", false).unwrap();
        clear_on_resume_in(&dir, "s2");
        assert!(read_in(&dir, "s2").is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn index_skips_junk() {
        let dir = tmpdir("index");
        set_outcome_in(&dir, "good", "/ws", Some(TaskOutcome::Completed), "c", false).unwrap();
        std::fs::write(dir.join("bad.json"), "not json").unwrap();
        std::fs::write(dir.join("ignored.txt"), "{}").unwrap();
        let idx = outcome_index(&dir);
        assert_eq!(idx.len(), 1);
        assert!(idx.contains_key("good"));
        std::fs::remove_dir_all(&dir).ok();
    }
}
