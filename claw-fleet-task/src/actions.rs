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
use std::path::PathBuf;

use crate::master::spawn_spec_from_task;
use crate::pitem::{FailReason, PItemId, PItemStatus};
use crate::runner::TaskHost;
use crate::task::{
    get_task, git_create_branch, pick_unique_branch, slugify_title, task_json_path,
    task_write_lock, tasks_dir, write_task_atomic, TaskStatus,
};
use crate::worker::worker_spawn_spec;
use crate::worktree::{self, MergeOutcome};

/// Optional knobs for `start_task`. Today only carries `force_human_gate_all`
/// (project-wide manual-review policy in claw-fleet-core); fleet-task's
/// binary always passes `Default::default()`.
#[derive(Debug, Clone, Default)]
pub struct StartOpts {
    /// When true, mark every P-item as `human_gate=true` before spawning the
    /// master. Used by `claw_fleet_core` to honour `Project.manual_review_all`.
    pub force_human_gate_all: bool,
}

/// Transition a Drafting / Planning / Ready task to Running. Creates a fresh
/// `fleet/<slug>` git branch in the workspace returned by
/// `host.workspace_for_task`, spawns the Master session via
/// `host.enqueue_master`, stamps `started_at` + `master_session_id`, and
/// persists `task_branch`. Idempotent: returns `Ok` immediately if the task
/// is already running.
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
    git_create_branch(&workspace, &branch)?;
    task.task_branch = Some(branch);
    task.status = TaskStatus::Running;
    task.started_at = Some(chrono::Utc::now().timestamp());
    if opts.force_human_gate_all {
        for item in task.plan.items.values_mut() {
            item.human_gate = true;
        }
    }
    write_task_atomic(&task)?;
    let spec = spawn_spec_from_task(&task, workspace.clone())?;
    match host.enqueue_master(&spec) {
        Ok(master_sid) => {
            task.master_session_id = Some(master_sid);
            write_task_atomic(&task)?;
            Ok(())
        }
        Err(e) => Err(format!("master enqueue failed: {e}")),
    }
}

/// Mark a P-item Done and persist its acceptance-audit summary. On a real
/// 3-way conflict, asks the host's `LlmMediator` to resolve the files; on
/// success the merge is finalised. The P-item's worktree is reaped.
pub fn mark_done(
    task_id: &str,
    p_item_id: &str,
    summary: &str,
    host: &dyn TaskHost,
) -> Result<MergeOutcome, String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    let workspace = host.workspace_for_task(&task)?;
    let task_branch = task
        .task_branch
        .clone()
        .ok_or_else(|| format!("task {task_id} has no task_branch"))?;
    let outcome = worktree::merge_back(&workspace, &task_branch, task_id, p_item_id)?;
    let outcome = if let MergeOutcome::Conflict { files, reason } = outcome {
        let mediations = host.resolve_conflicts(&files).map_err(|e| {
            format!(
                "mediator failed for {n} conflicted file(s) ({reason}): {e}",
                n = files.len(),
            )
        })?;
        let resolutions: Vec<(PathBuf, String)> = mediations
            .into_iter()
            .map(|m| (m.path, m.resolved_content))
            .collect();
        worktree::apply_resolutions(&workspace, task_id, p_item_id, &resolutions)?
    } else {
        outcome
    };
    let now = chrono::Utc::now().timestamp();
    {
        let item = task
            .plan
            .get_mut(p_item_id)
            .ok_or_else(|| format!("p-item {p_item_id} not found in task {task_id}"))?;
        item.status = PItemStatus::Done;
        item.completed_at = Some(now);
        item.output_summary = Some(summary.to_string());
    }
    write_task_atomic(&task)?;
    let _ = worktree::reap(&workspace, task_id, p_item_id);
    Ok(outcome)
}

/// Mark a P-item Failed. Releases the worktree, flips the status, propagates
/// skip on poisoned downstream items.
pub fn mark_failed(
    task_id: &str,
    p_item_id: &str,
    reason: FailReason,
    host: &dyn TaskHost,
) -> Result<Vec<PItemId>, String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    let now = chrono::Utc::now().timestamp();
    {
        let item = task
            .plan
            .get_mut(p_item_id)
            .ok_or_else(|| format!("p-item {p_item_id} not found in task {task_id}"))?;
        item.status = PItemStatus::Failed(reason);
        item.completed_at = Some(now);
    }
    let newly_skipped = task.plan.propagate_skip();
    write_task_atomic(&task)?;
    if let Ok(workspace) = host.workspace_for_task(&task) {
        let _ = worktree::reap(&workspace, task_id, p_item_id);
    }
    Ok(newly_skipped)
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
            PItemStatus::WaitDeps | PItemStatus::WaitResource => {
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
