//! Thin wrappers around `claw_fleet_task::actions::*` that bind the
//! supervisor-backed `SupervisorHost` so the existing
//! `claw_fleet_core::task::start_task(&id)` import paths from desktop /
//! fleet-cli continue to work without changes.
//!
//! Phase 2 moved the actual business logic into `claw_fleet_task::actions`
//! so the standalone `fleet-task` binary can share it via a binary-local
//! host. `reconcile_task_completion` stays here because it reads the legacy
//! `fleet-sessions.json` table that fleet-task explicitly doesn't use.

use claw_fleet_task::actions;
use claw_fleet_task::task::{get_task, list_tasks, task_write_lock, write_task_atomic, TaskStatus};

use crate::lifecycle_host::SupervisorHost;

pub fn start_task(task_id: &str) -> Result<(), String> {
    // Look up the project once so we can honour `manual_review_all` — the
    // host trait doesn't carry project metadata.
    let task = get_task(task_id)?;
    let project = crate::project::list_projects()
        .into_iter()
        .find(|p| p.id == task.project_id)
        .ok_or_else(|| format!("project {} not found for task {task_id}", task.project_id))?;
    let opts = actions::StartOpts {
        force_human_gate_all: project.manual_review_all,
    };
    actions::start_task(task_id, &SupervisorHost, opts)
}

pub fn dispatch_pitem(task_id: &str, p_item_id: &str) -> Result<(), String> {
    actions::dispatch_pitem(task_id, p_item_id, &SupervisorHost)
}

pub fn pause_task(task_id: &str) -> Result<(), String> {
    actions::pause_task(task_id, &SupervisorHost)
}

pub fn resume_task(task_id: &str) -> Result<(), String> {
    actions::resume_task(task_id, &SupervisorHost)
}

pub fn clear_task(task_id: &str) -> Result<(), String> {
    actions::clear_task(task_id, &SupervisorHost)
}

/// P7 final acceptance: user confirms a task parked in `AwaitingAcceptance`,
/// flipping it to `Done`. No host needed — the orchestrator already finished.
pub fn accept_task(task_id: &str) -> Result<(), String> {
    actions::accept_task(task_id)
}

/// Reconcile Task status against its master FleetSession on disk. Called by
/// the supervisor tick: if the master session has exited AND the task's plan
/// is fully terminal, flip the task to Done. Returns the number of tasks
/// that transitioned.
///
/// Stays in core (rather than migrating to claw-fleet-task::actions) because
/// it consults the legacy `fleet-sessions.json` table — fleet-task's
/// LocalHost path tracks master exit via its in-process pid HashMap, so it
/// has no use for this function.
pub fn reconcile_task_completion() -> Result<usize, String> {
    let sessions = crate::project::list_fleet_sessions();
    let tasks = list_tasks(None);
    let mut transitioned = 0usize;
    for task in tasks {
        if !matches!(task.status, TaskStatus::Running) {
            continue;
        }
        let Some(master_sid) = task.master_session_id.as_deref() else {
            continue;
        };
        let Some(master) = sessions.iter().find(|s| s.id == master_sid) else {
            continue;
        };
        let master_finished =
            master.status == crate::project::DEFAULT_COLUMN_COMPLETE && master.pid.is_none();
        if !master_finished {
            continue;
        }
        if !task.is_plan_finished() {
            continue;
        }
        let lock = task_write_lock(&task.id);
        let _g = lock.lock().expect("task write mutex poisoned");
        let Ok(mut latest) = get_task(&task.id) else { continue };
        if matches!(latest.status, TaskStatus::Running) && latest.is_plan_finished() {
            latest.status = TaskStatus::Done;
            latest.completed_at = Some(chrono::Utc::now().timestamp());
            if write_task_atomic(&latest).is_ok() {
                transitioned += 1;
            }
        }
    }
    Ok(transitioned)
}
