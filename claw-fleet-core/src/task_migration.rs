//! The one-shot migration into the task model (projects ⊃ tasks ⊃ sub-tasks).
//!
//! Most of the model needs no migration: [`crate::task_model`] derives task
//! identity from handoff chains and spawn edges at scan time, and an old
//! spawn with no recorded parent simply becomes a task of its own. Two things
//! do need writing once:
//!
//! 1. **Directory → project.** Every code directory a Fleet-launched session
//!    ran in becomes a project in [`crate::project_registry`]. Forks are side
//!    questions, not work in a directory, and are skipped.
//! 2. **Done marks → outcomes.** The task list used to close work with the
//!    session-list `Done` mark; the five task states read only the task
//!    outcome. A `Done` session with no outcome would otherwise land in
//!    "awaiting review" — on the machine this was written on, 3,286 of them.
//!    Each gets a `Completed` outcome with card id [`MIGRATION_CARD_ID`] and
//!    `agent_claimed_complete = false`, so the retrospective can tell these
//!    apart from a press of the end-task button. The plan reviver already
//!    treats a `Done` mark as finished, so nothing wakes or sleeps differently.
//!
//! Recorded by a marker under `~/.fleet/migrations/`; both steps are
//! idempotent, so a failure part-way simply runs again next start.

use std::collections::HashMap;
use std::path::Path;

use crate::control_plane::Step;

const MARKER: &str = "task-model-v1.done";

/// The `card_id` stamped on outcomes this migration writes.
pub const MIGRATION_CARD_ID: &str = "migration";

fn marker() -> Option<std::path::PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join("migrations").join(MARKER))
}

/// Run the migration unless it already ran. Returns the steps it took.
pub fn run() -> Vec<Step> {
    let Some(marker) = marker() else {
        return Vec::new();
    };
    if marker.exists() {
        return Vec::new();
    }
    let notes = crate::launch_spec::all();
    let steps = vec![
        Step {
            name: "projects",
            result: seed_projects(&notes).map(|n| {
                crate::log_debug(&format!("task migration: registered {n} project(s)"));
            }),
        },
        Step {
            name: "done_outcomes",
            result: match (crate::session_mark::mark_dir(), crate::task_outcome::outcome_dir()) {
                (Some(marks), Some(outcomes)) => backfill_outcomes(&marks, &outcomes).map(|n| {
                    crate::log_debug(&format!("task migration: closed {n} done-marked session(s)"));
                }),
                _ => Err("cannot determine home dir".into()),
            },
        },
    ];
    if steps.iter().all(|s| s.result.is_ok()) {
        let stamp = chrono::Utc::now().to_rfc3339();
        let written = marker
            .parent()
            .map(std::fs::create_dir_all)
            .unwrap_or(Ok(()))
            .and_then(|_| std::fs::write(&marker, stamp));
        if let Err(e) = written {
            crate::log_debug(&format!("task migration: write marker failed: {e}"));
        }
    }
    steps
}

pub fn run_and_log(caller: &str) {
    for step in run() {
        if let Err(e) = &step.result {
            crate::log_debug(&format!("task migration ({caller}): {} failed: {e}", step.name));
        }
    }
}

fn seed_projects(notes: &[(String, crate::launch_spec::LaunchSpec)]) -> Result<usize, String> {
    crate::project_registry::ensure_many(
        notes
            .iter()
            .filter(|(_, n)| n.kind.as_deref() != Some("fork"))
            .filter_map(|(_, n)| n.workspace.as_deref()),
    )
}

/// Give every `Done`-marked session without an outcome a `Completed` one.
/// Returns how many were written.
fn backfill_outcomes(marks: &Path, outcomes: &Path) -> Result<usize, String> {
    use crate::session_mark::SessionMark;
    use crate::task_outcome::TaskOutcome;

    let existing = crate::task_outcome::outcome_index(outcomes);
    let done: HashMap<String, String> = crate::session_mark::mark_index_records(marks)
        .into_iter()
        .filter(|(_, r)| r.mark == SessionMark::Done)
        .map(|(id, r)| (id, r.workspace_path))
        .collect();
    let mut written = 0;
    for (id, workspace) in done {
        if existing.contains_key(&id) {
            continue;
        }
        crate::task_outcome::set_outcome_in(
            outcomes,
            &id,
            &workspace,
            Some(TaskOutcome::Completed),
            MIGRATION_CARD_ID,
            false,
        )?;
        written += 1;
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session_mark::SessionMark;
    use crate::task_outcome::TaskOutcome;

    #[test]
    fn done_marks_without_outcome_become_completed() {
        let base = tempfile::tempdir().unwrap();
        let marks = base.path().join("session-mark");
        let outcomes = base.path().join("task-outcome");
        crate::session_mark::set_mark_in(&marks, "done-1", "/w", Some(SessionMark::Done)).unwrap();
        crate::session_mark::set_mark_in(&marks, "pending-1", "/w", Some(SessionMark::Pending))
            .unwrap();
        crate::session_mark::set_mark_in(&marks, "abandoned-1", "/w", Some(SessionMark::Done))
            .unwrap();
        crate::task_outcome::set_outcome_in(
            &outcomes,
            "abandoned-1",
            "/w",
            Some(TaskOutcome::Abandoned),
            "card-x",
            true,
        )
        .unwrap();

        assert_eq!(backfill_outcomes(&marks, &outcomes).unwrap(), 1);

        let idx = crate::task_outcome::outcome_index(&outcomes);
        let rec = &idx["done-1"];
        assert_eq!(rec.outcome, TaskOutcome::Completed);
        assert_eq!(rec.card_id, MIGRATION_CARD_ID);
        assert!(!rec.agent_claimed_complete);
        assert_eq!(rec.workspace_path, "/w");
        // An existing outcome is never overwritten, and Pending is left open.
        assert_eq!(idx["abandoned-1"].outcome, TaskOutcome::Abandoned);
        assert!(!idx.contains_key("pending-1"));

        // Second run is a no-op.
        assert_eq!(backfill_outcomes(&marks, &outcomes).unwrap(), 0);
    }
}
