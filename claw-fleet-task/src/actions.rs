//! Business actions on `Task` — the side-effectful operations that
//! coordinate master/worker spawn, worktree merge, and the host-provided
//! subprocess + LLM hooks. Pure data + persistence stays in `crate::task`.
//!
//! Migrated from `claw_fleet_core::task_actions` so the standalone
//! `fleet-task` binary can reuse the exact same lifecycle logic. The
//! callbacks the actions used to make into `crate::project` /
//! `crate::supervisor` / `crate::merge_mediator` are now expressed via the
//! `TaskHost` trait (`TaskLifecycleHost + LlmMediator`).
//!
//! `claw_fleet_core::task_actions` keeps thin wrappers around these so the
//! existing `claw_fleet_core::task::start_task(&id)` import path continues
//! to work — those wrappers supply a `SupervisorHost` host transparently.
//!
//! `reconcile_task_completion` did **not** migrate: it's tied to the legacy
//! `fleet-sessions.json` table that fleet-task explicitly does not use.

use std::fs;

use crate::pitem::PItemStatus;
use crate::runner::TaskHost;
use crate::task::{
    get_task, git_create_branch, pick_unique_branch, slugify_title, task_json_path,
    task_write_lock, tasks_dir, write_task_atomic, TaskStatus,
};
use crate::worker::worker_spawn_spec;
use crate::worktree;

/// Optional knobs for `start_task`. Today only carries `force_human_gate_all`
/// (project-wide manual-review policy in claw-fleet-core); fleet-task's
/// binary always passes `Default::default()`.
#[derive(Debug, Clone, Default)]
pub struct StartOpts {
    /// When true, mark every P-item as `human_gate=true` before spawning the
    /// master. Used by `claw_fleet_core` to honour `Project.manual_review_all`.
    pub force_human_gate_all: bool,
}

/// Prepare a Drafting task for the deterministic orchestrator and flip it to
/// Running. Creates a fresh `fleet/<slug>` git branch in the workspace returned
/// by `host.workspace_for_task`, persists `task_branch` + `workspace`, stamps
/// `started_at`, and flips `status` to Running. Idempotent: returns `Ok`
/// immediately if the task is already running.
///
/// Rebuilt (task-subsystem-rebuild): this no longer spawns an LLM master — the
/// `fleet-task` process drives the plan via `orchestrator::Orchestrator::step`.
/// `start_task` is now pure task-prep; the `host` is used only to resolve the
/// workspace.
pub fn start_task(task_id: &str, host: &dyn TaskHost, opts: StartOpts) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    if matches!(task.status, TaskStatus::Running) {
        return Ok(());
    }
    let workspace = host.workspace_for_task(&task)?;
    let slug = slugify_title(&task.title);
    let branch = pick_unique_branch(&workspace, &slug)?;
    // Record the branch we're forking from BEFORE git_create_branch switches
    // HEAD to fleet/<slug> — accept_task's merge-back targets this branch.
    if task.base_branch.is_none() {
        task.base_branch = crate::task::Task::current_branch(&workspace);
    }
    git_create_branch(&workspace, &branch)?;
    task.task_branch = Some(branch);
    // Persist workspace so out-of-process tools (fleet-cli, the orchestrator)
    // can find the working tree without a project-table lookup.
    task.workspace = Some(workspace.clone());
    task.status = TaskStatus::Running;
    task.started_at = Some(chrono::Utc::now().timestamp());
    if opts.force_human_gate_all {
        for item in task.plan.items.values_mut() {
            item.human_gate = true;
        }
    }
    write_task_atomic(&task)?;
    Ok(())
}


/// Master tool — dispatch worker for `p_item_id`. Flips its status to
/// Running, provisions a worktree under the task branch, and asks the host
/// to spawn a Worker session via `enqueue_worker`.
pub fn dispatch_pitem(
    task_id: &str,
    p_item_id: &str,
    host: &dyn TaskHost,
) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    let now = chrono::Utc::now().timestamp();
    {
        let item = task
            .plan
            .get_mut(p_item_id)
            .ok_or_else(|| format!("p-item {p_item_id} not found in task {task_id}"))?;
        match &item.status {
            PItemStatus::WaitDeps => {
                item.status = PItemStatus::Running;
                item.started_at = Some(now);
            }
            PItemStatus::Running => {
                return Ok(());
            }
            other => {
                return Err(format!(
                    "cannot dispatch p-item {p_item_id} in status {other:?}"
                ));
            }
        }
    }
    write_task_atomic(&task)?;
    let workspace = host.workspace_for_task(&task)?;
    let task_branch = task.task_branch.as_deref().ok_or_else(|| {
        format!("task {task_id} has no task_branch — call start_task before dispatching P-items")
    })?;
    let cwd = worktree::provision(&workspace, task_branch, task_id, p_item_id)?;
    let spec = worker_spawn_spec(&task, p_item_id, cwd)?;
    let worker_sid = host.enqueue_worker(&spec)?;
    if let Some(item) = task.plan.get_mut(p_item_id) {
        item.agent_session_id = Some(worker_sid);
    }
    write_task_atomic(&task)
}

/// Pause a running task. Flips status to Paused and asks the host to SIGSTOP
/// all related subprocesses. Signal failures are non-fatal.
pub fn pause_task(task_id: &str, host: &dyn TaskHost) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    if matches!(task.status, TaskStatus::Paused) {
        return Ok(());
    }
    if !matches!(task.status, TaskStatus::Running) {
        return Err(format!(
            "task {task_id} is not running (status: {:?})",
            task.status
        ));
    }
    task.status = TaskStatus::Paused;
    write_task_atomic(&task)?;
    let _ = host.pause_task_sessions(task_id);
    Ok(())
}

/// Resume a paused task — host SIGCONT and flip Running.
pub fn resume_task(task_id: &str, host: &dyn TaskHost) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    if matches!(task.status, TaskStatus::Running) {
        return Ok(());
    }
    if !matches!(task.status, TaskStatus::Paused) {
        return Err(format!(
            "task {task_id} is not paused (status: {:?})",
            task.status
        ));
    }
    task.status = TaskStatus::Running;
    write_task_atomic(&task)?;
    let _ = host.resume_task_sessions(task_id);
    Ok(())
}

/// Final acceptance (P7): the user confirms a fully-reviewed task. Flips
/// `AwaitingAcceptance` → `Done` and stamps `completed_at`. Idempotent on an
/// already-Done task; errors if the task is not awaiting acceptance (you can't
/// accept something still running / drafting). No host signalling needed — the
/// orchestrator already finished and reaped every worker before parking the
/// task in `AwaitingAcceptance`.
pub fn accept_task(task_id: &str) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    if matches!(task.status, TaskStatus::Done) {
        return Ok(());
    }
    if !matches!(task.status, TaskStatus::AwaitingAcceptance) {
        return Err(format!(
            "task {task_id} is not awaiting acceptance (status: {:?})",
            task.status
        ));
    }
    task.status = TaskStatus::Done;
    task.completed_at = Some(chrono::Utc::now().timestamp());
    write_task_atomic(&task)
}

/// Clear (delete) a task — host SIGTERM, then remove task json + materials.
pub fn clear_task(task_id: &str, host: &dyn TaskHost) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let _ = host.terminate_task_sessions(task_id);
    if let Ok(mut task) = get_task(task_id) {
        task.status = TaskStatus::Abandoned;
        let _ = write_task_atomic(&task);
    }
    let json = task_json_path(task_id)?;
    if json.exists() {
        fs::remove_file(&json).map_err(|e| format!("remove task json: {e}"))?;
    }
    let materials = tasks_dir()?.join(task_id);
    if materials.exists() {
        fs::remove_dir_all(&materials).map_err(|e| format!("remove materials dir: {e}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::Task;

    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }
    impl FleetHomeOverride {
        fn new(tmp: &std::path::Path) -> Self {
            let lock = crate::paths::fleet_home_lock();
            let prev = std::env::var_os("FLEET_HOME");
            std::env::set_var("FLEET_HOME", tmp);
            FleetHomeOverride { prev, _lock: lock }
        }
    }
    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            match &self.prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
    }

    #[test]
    fn accept_task_flips_awaiting_acceptance_to_done() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());
        let mut task = Task::drafting("t-acc".into(), "p".into(), "x".into(), 0);
        task.status = TaskStatus::AwaitingAcceptance;
        write_task_atomic(&task).unwrap();

        accept_task("t-acc").unwrap();
        let after = get_task("t-acc").unwrap();
        assert!(matches!(after.status, TaskStatus::Done));
        assert!(after.completed_at.is_some());
        // Idempotent on an already-Done task.
        assert!(accept_task("t-acc").is_ok());
    }

    #[test]
    fn accept_task_rejects_non_awaiting() {
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());
        let mut task = Task::drafting("t-run".into(), "p".into(), "x".into(), 0);
        task.status = TaskStatus::Running;
        write_task_atomic(&task).unwrap();
        assert!(accept_task("t-run").is_err(), "can't accept a still-running task");
    }
}
