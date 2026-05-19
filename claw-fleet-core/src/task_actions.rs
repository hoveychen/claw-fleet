//! Business actions on `Task` — the side-effectful operations that
//! coordinate master/worker spawn, worktree merge, and the supervisor.
//! Pure data + persistence lives in `claw_fleet_task::task`.

use std::fs;
use std::path::PathBuf;

use claw_fleet_task::task::{
    get_task, git_create_branch, list_tasks, pick_unique_branch, slugify_title, task_json_path,
    task_write_lock, tasks_dir, write_task_atomic, TaskStatus,
};

/// Transition a Drafting / Planning / Ready task to Running. Creates a fresh
/// `fleet/<slug>` git branch in the project workspace, spawns the Master
/// session (PRD §5.7 / TASKS P19), stamps `started_at` + `master_session_id`,
/// and persists `task_branch`. Idempotent: returns `Ok` immediately if the
/// task is already running.
///
/// **Master spawn integration** (TASKS P19 follow-up):
/// On the happy path this also calls `supervisor::enqueue_master` so a
/// Sonnet-4.6 Claude Code subprocess starts on next tick. If the spawn enqueue
/// fails, the task still transitions to Running (branch is created, plan is
/// editable) but `master_session_id` stays None and the caller surfaces the
/// error — recovery is `pause` + `resume` once the underlying issue is fixed.
pub fn start_task(task_id: &str) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    if matches!(task.status, TaskStatus::Running) {
        return Ok(());
    }
    let project = crate::project::list_projects()
        .into_iter()
        .find(|p| p.id == task.project_id)
        .ok_or_else(|| format!("project {} not found for task {task_id}", task.project_id))?;
    let workspace = PathBuf::from(&project.workspace);
    let slug = slugify_title(&task.title);
    let branch = pick_unique_branch(&workspace, &slug)?;
    git_create_branch(&workspace, &branch)?;
    task.task_branch = Some(branch);
    task.status = TaskStatus::Running;
    task.started_at = Some(chrono::Utc::now().timestamp());
    // PRD §5.x / TASKS P14: if the project has manual_review_all on, force
    // every P-item to require human gate so the master pauses for user
    // confirmation before marking any P-item Done.
    if project.manual_review_all {
        for item in task.plan.items.values_mut() {
            item.human_gate = true;
        }
    }
    // Persist the Running + branch state first so `enqueue_master`'s
    // internal `get_task` sees the up-to-date task (it reads from disk).
    write_task_atomic(&task)?;
    // Spawn the master. Failure here is non-fatal to the state transition —
    // the task is Running with a branch; user can pause/resume to retry.
    let spec = crate::master::spawn_spec_from_task(&task, workspace.clone())?;
    match crate::supervisor::enqueue_master(&spec) {
        Ok(master_sid) => {
            task.master_session_id = Some(master_sid);
            write_task_atomic(&task)?;
            Ok(())
        }
        Err(e) => Err(format!("master enqueue failed: {e}")),
    }
}

// ── Master-callable mutations ────────────────────────────────────────────────

/// Mark a P-item Done and persist its acceptance-audit summary. Per PRD §5.7
/// only the Master should call this — the underlying mutex is the same one
/// `update_plan` uses, so concurrent calls are serialised.
///
/// V2: also fast-forward-merges the P-item's worktree branch back into the
/// task branch and reaps the worktree. If the merge cannot fast-forward
/// (worker branch diverged from task branch), the status flip is rolled
/// back and a `Conflict` outcome is returned — the master is expected to
/// react (Phase 2 will plug in LLM mediation here).
pub fn mark_done(
    task_id: &str,
    p_item_id: &str,
    summary: &str,
) -> Result<crate::worktree::MergeOutcome, String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    let project = crate::project::list_projects()
        .into_iter()
        .find(|p| p.id == task.project_id)
        .ok_or_else(|| format!("project {} not found for task {task_id}", task.project_id))?;
    let workspace = PathBuf::from(&project.workspace);
    let task_branch = task
        .task_branch
        .clone()
        .ok_or_else(|| format!("task {task_id} has no task_branch"))?;
    let outcome = crate::worktree::merge_back(&workspace, &task_branch, task_id, p_item_id)?;
    // Phase 2 mediation: on a real 3-way conflict, ask Sonnet to produce
    // resolved content, apply it, and re-commit the merge. If mediation
    // fails (provider unavailable, response unusable, etc.) bubble the
    // error to the master so it can retry mark-done or escalate to the
    // user via AskUserQuestion.
    let outcome = if let crate::worktree::MergeOutcome::Conflict { files, reason } = outcome {
        let mediations = crate::merge_mediator::mediate(&files).map_err(|e| {
            format!(
                "mediator failed for {n} conflicted file(s) ({reason}): {e}",
                n = files.len(),
            )
        })?;
        let resolutions: Vec<(PathBuf, String)> = mediations
            .into_iter()
            .map(|m| (m.path, m.resolved_content))
            .collect();
        crate::worktree::apply_resolutions(&workspace, task_id, p_item_id, &resolutions)?
    } else {
        outcome
    };
    let now = chrono::Utc::now().timestamp();
    {
        let item = task
            .plan
            .get_mut(p_item_id)
            .ok_or_else(|| format!("p-item {p_item_id} not found in task {task_id}"))?;
        item.status = crate::pitem::PItemStatus::Done;
        item.completed_at = Some(now);
        item.output_summary = Some(summary.to_string());
    }
    // After flipping to Done, propagate skip on any newly poisoned downstream
    // (no-op for mark_done since Done unblocks; this is the symmetric place
    // mark_failed runs it).
    write_task_atomic(&task)?;
    // Worktree's commits are now in task_branch — reap to free disk + branch.
    let _ = crate::worktree::reap(&workspace, task_id, p_item_id);
    Ok(outcome)
}

/// Mark a P-item Failed. Per PRD §5.7 / TASKS P20 this **also releases the
/// resource lock immediately** — the master/supervisor must observe a failed
/// item as "not holding any resource" so a retry or sibling P-item isn't
/// stranded. Resource locks live in supervisor's runtime state (not on
/// disk); this function flips the status and runs `propagate_skip` so the
/// poison cascade is reflected on disk before the supervisor reads back.
pub fn mark_failed(
    task_id: &str,
    p_item_id: &str,
    reason: crate::pitem::FailReason,
) -> Result<Vec<crate::pitem::PItemId>, String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    let now = chrono::Utc::now().timestamp();
    {
        let item = task
            .plan
            .get_mut(p_item_id)
            .ok_or_else(|| format!("p-item {p_item_id} not found in task {task_id}"))?;
        item.status = crate::pitem::PItemStatus::Failed(reason);
        item.completed_at = Some(now);
    }
    let newly_skipped = task.plan.propagate_skip();
    write_task_atomic(&task)?;
    // Reap the failed P-item's worktree so disk doesn't leak. The branch
    // is preserved by the orphan-branch recovery in `worktree::provision`
    // if a retry comes around; reap deletes it but provision can rebuild.
    if let Some(project) = crate::project::list_projects()
        .into_iter()
        .find(|p| p.id == task.project_id)
    {
        let workspace = PathBuf::from(&project.workspace);
        let _ = crate::worktree::reap(&workspace, task_id, p_item_id);
    }
    Ok(newly_skipped)
}

/// Master tool — request the supervisor to dispatch worker for `p_item_id`.
/// Flips the P-item's status to `Running`, stamps `started_at`, and spawns
/// a fresh Worker session via `supervisor::enqueue_worker`. The Master
/// agent sees the status change + `agent_session_id` in subsequent
/// `get-plan` / `read-output` calls.
pub fn dispatch_pitem(task_id: &str, p_item_id: &str) -> Result<(), String> {
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
            crate::pitem::PItemStatus::WaitDeps | crate::pitem::PItemStatus::WaitResource => {
                item.status = crate::pitem::PItemStatus::Running;
                item.started_at = Some(now);
            }
            crate::pitem::PItemStatus::Running => {
                // Idempotent — already running. Don't double-spawn.
                return Ok(());
            }
            other => {
                return Err(format!(
                    "cannot dispatch p-item {p_item_id} in status {other:?}"
                ));
            }
        }
    }
    // Persist Running state first so the worker's spawn read sees it on disk.
    write_task_atomic(&task)?;
    // Build worker spawn spec from the task + project workspace. V2: each
    // P-item runs in its own git worktree rooted at the task branch so
    // parallel workers can build/test without trampling each other.
    let project = crate::project::list_projects()
        .into_iter()
        .find(|p| p.id == task.project_id)
        .ok_or_else(|| format!("project {} not found for task {task_id}", task.project_id))?;
    let workspace = PathBuf::from(&project.workspace);
    let task_branch = task.task_branch.as_deref().ok_or_else(|| {
        format!("task {task_id} has no task_branch — call start_task before dispatching P-items")
    })?;
    let cwd = crate::worktree::provision(&workspace, task_branch, task_id, p_item_id)?;
    let spec = crate::worker_executor::worker_spawn_spec(&task, p_item_id, cwd)?;
    let worker_sid = crate::supervisor::enqueue_worker(&spec)?;
    // Stamp the worker session id on the P-item so the master can locate
    // the transcript via `fleet task read-output`.
    if let Some(item) = task.plan.get_mut(p_item_id) {
        item.agent_session_id = Some(worker_sid);
    }
    write_task_atomic(&task)
}

// ── User-only operations (master has no tools for these) ─────────────────────

/// Pause a running task. Flips Task.status to `Paused`, persists, and asks
/// supervisor to SIGSTOP the master + any in-flight workers (TASKS P20).
/// Signal failures are non-fatal — the disk state still flips, and the
/// supervisor's next tick reconciles.
pub fn pause_task(task_id: &str) -> Result<(), String> {
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
    let _ = crate::supervisor::pause_task_sessions(task_id);
    Ok(())
}

/// Resume a paused task — SIGCONT master + workers, flip Running.
pub fn resume_task(task_id: &str) -> Result<(), String> {
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
    let _ = crate::supervisor::resume_task_sessions(task_id);
    Ok(())
}

/// Clear (delete) a task — terminates master + workers, removes both the
/// task json and the materials dir. Frees all resource locks immediately.
pub fn clear_task(task_id: &str) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    // Tear down running subprocesses first so they don't keep writing to a
    // workspace that's about to be unbound from any task record.
    let _ = crate::supervisor::terminate_task_sessions(task_id);
    // Mark Abandoned next so any in-flight supervisor reads see the
    // intent before the file vanishes.
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

/// Reconcile Task status against its master FleetSession on disk. Called by
/// the supervisor tick: if the master session has exited (status=complete
/// or pid no longer alive) AND the task's plan is fully terminal, flip the
/// task to Done and stamp `completed_at`. Returns the number of tasks that
/// transitioned.
///
/// This is the disk-only reconciler — the actual master subprocess exit
/// detection lives in `supervisor::tick`. Separating the two keeps tests
/// hermetic (no real subprocesses).
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
        // Re-read in case another writer raced ahead.
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
