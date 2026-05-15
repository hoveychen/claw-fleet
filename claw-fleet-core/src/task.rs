//! `Task` — the run unit one level above a `Project` and one level below
//! a `PItem`. Persisted at `~/.fleet/tasks/<id>.json` by the LocalBackend
//! (see PRD §6.1, design/task-as-unit-redesign.md).
//!
//! Inbox materials are stored at `~/.fleet/tasks/<id>/materials/<name>` —
//! `Material::File.path` is an absolute pathname to that location, not the
//! original user path.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};

use crate::pitem::{PItemId, PItemStatus};
use crate::plan::DagPlan;
use crate::session::get_fleet_dir;

pub type TaskId = String;
pub type ProjectId = String;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    pub id: TaskId,
    pub project_id: ProjectId,
    pub title: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub inbox_materials: Vec<Material>,
    #[serde(default)]
    pub plan: DagPlan,
    pub status: TaskStatus,
    pub created_at: i64,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub task_branch: Option<String>,
    /// `claw-fleet-core::session::SessionId` of the master agent running this
    /// task. `None` before start, `Some` once `start_task` spawns master.
    #[serde(default)]
    pub master_session_id: Option<String>,
    /// `true` if `title` is a system-generated placeholder that should be
    /// replaced by the master session's `aiTitle` once available. Flips to
    /// `false` the moment the user edits the title manually. Legacy task
    /// JSONs (where the field is missing) deserialize to `false` so the
    /// human-typed title from the old InboxDialog is never overwritten.
    #[serde(default)]
    pub title_auto: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    /// User is still drafting in Inbox (no plan yet).
    Drafting,
    /// Planner is generating the DAG.
    Planning,
    /// Plan generated, awaiting `start_task`.
    Ready,
    /// Master + workers actively running.
    Running,
    /// `pause` issued — master + workers SIGSTOPped.
    Paused,
    /// All P-items terminal (Done / Skipped / Failed) and master finalised.
    Done,
    /// User invoked `clear` — kept around briefly for audit before deletion.
    Abandoned,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum Material {
    /// A file the user dropped into the Inbox. `path` is the
    /// fleet-managed absolute path under `~/.fleet/tasks/<id>/materials/`,
    /// not the source file the user dragged from.
    File {
        path: PathBuf,
        media: MediaKind,
        added_at: i64,
    },
    /// Free text or a pasted snippet.
    Text {
        content: String,
        added_at: i64,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MediaKind {
    Document,
    Image,
    Screenshot,
    Other,
}

/// Backend `create_task` payload. Materials get added after creation through
/// a separate `add_task_material` call (lands in P3 with LocalBackend).
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TaskInput {
    pub project_id: ProjectId,
    /// Optional in the wire format. When blank, `create_task` derives a
    /// placeholder from `description` (or "未命名任务" if both are empty)
    /// and flags the resulting task `title_auto = true` so the master
    /// session's aiTitle later overwrites it.
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub description: String,
}

/// SSE events pushed by `subscribe_task_events`. The Master agent's
/// event-router (P21) translates these to `[event] ...` append-user-message
/// calls; the UI uses them to refresh the Kanban without polling.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum TaskEvent {
    /// `task.status` changed (e.g. `ready` → `running`).
    TaskStatusChanged { task_id: TaskId, status: TaskStatus },
    /// One P-item's status changed (worker started, finished, gated, etc.).
    PItemStatusChanged {
        task_id: TaskId,
        p_item_id: PItemId,
        status: PItemStatus,
    },
    /// Master called `update-plan` and the DAG shape is now different.
    /// Subscribers should refetch the task to pick up the new plan.
    PlanUpdated { task_id: TaskId },
    /// User added Inbox material to a not-yet-started task.
    MaterialAdded { task_id: TaskId },
    /// A worker emitted a status note or the Master appended an `[event]` /
    /// `[user]` message. The content is what the UI shows verbatim.
    MasterMessage {
        task_id: TaskId,
        source: MasterMessageSource,
        message: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum MasterMessageSource {
    /// `[event] ...` — supervisor / scheduler / worker notification.
    Event,
    /// `[user] ...` — user-appended message via GUI / CLI.
    User,
    /// Master agent's own status / progress note.
    Master,
}

impl Task {
    /// Helper used by callers that draft a fresh task from the Inbox before
    /// the planner runs. Keeps the default-shape construction in one place.
    pub fn drafting(id: TaskId, project_id: ProjectId, title: String, created_at: i64) -> Self {
        Self {
            id,
            project_id,
            title,
            description: String::new(),
            inbox_materials: Vec::new(),
            plan: DagPlan::default(),
            status: TaskStatus::Drafting,
            created_at,
            started_at: None,
            completed_at: None,
            task_branch: None,
            master_session_id: None,
            title_auto: false,
        }
    }

    /// `true` when no P-item is still pending or active.
    pub fn is_plan_finished(&self) -> bool {
        !self.plan.is_empty() && self.plan.items.values().all(|p| p.is_terminal())
    }
}

// ── Storage layer ────────────────────────────────────────────────────────────
//
// On disk:
//   ~/.fleet/tasks/<id>.json                     — task metadata
//   ~/.fleet/tasks/<id>/materials/<filename>    — inbox materials (file content)
//
// Concurrent writes serialise through `task_write_lock(id)` per PRD §5.7
// (Task state concurrent write lock). The master agent + N workers can call
// `update_plan` / `add_task_material` from different threads; per-task mutex
// guarantees the json file is never observed mid-write.

fn task_locks() -> &'static Mutex<HashMap<TaskId, Arc<Mutex<()>>>> {
    static LOCKS: OnceLock<Mutex<HashMap<TaskId, Arc<Mutex<()>>>>> = OnceLock::new();
    LOCKS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn task_write_lock(id: &str) -> Arc<Mutex<()>> {
    let mut map = task_locks().lock().expect("task locks map poisoned");
    map.entry(id.to_string())
        .or_insert_with(|| Arc::new(Mutex::new(())))
        .clone()
}

/// `~/.fleet/tasks/`. Errors if the fleet home cannot be resolved.
pub fn tasks_dir() -> Result<PathBuf, String> {
    get_fleet_dir()
        .map(|f| f.join("tasks"))
        .ok_or_else(|| "could not resolve ~/.fleet directory".to_string())
}

/// `~/.fleet/tasks/<id>.json`
pub fn task_json_path(task_id: &str) -> Result<PathBuf, String> {
    Ok(tasks_dir()?.join(format!("{task_id}.json")))
}

/// `~/.fleet/tasks/<id>/materials/`
pub fn task_materials_dir(task_id: &str) -> Result<PathBuf, String> {
    Ok(tasks_dir()?.join(task_id).join("materials"))
}

fn write_task_atomic(task: &Task) -> Result<(), String> {
    let path = task_json_path(&task.id)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("create tasks dir: {e}"))?;
    }
    let json = serde_json::to_string_pretty(task).map_err(|e| format!("serialize task: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("write temp task json: {e}"))?;
    fs::rename(&tmp, &path).map_err(|e| format!("rename task json: {e}"))?;
    Ok(())
}

/// Create a new task in `Drafting` status. Persists `<id>.json` and creates
/// the `materials/` directory. The `task_id` is a fresh UUID v4.
///
/// Title resolution:
/// - `input.title` is `Some(non-blank)` → use verbatim, `title_auto = false`.
/// - Otherwise → derive a placeholder from `description` (first line, ≤60
///   chars), or `"未命名任务"` if both are blank, and set `title_auto = true`
///   so the master session's aiTitle later overwrites it.
pub fn create_task(input: TaskInput) -> Result<Task, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let (title, title_auto) = resolve_initial_title(&input.title, &input.description);
    let mut task = Task::drafting(id, input.project_id, title, now);
    task.description = input.description;
    task.title_auto = title_auto;
    fs::create_dir_all(task_materials_dir(&task.id)?).map_err(|e| format!("mkdir materials: {e}"))?;
    write_task_atomic(&task)?;
    Ok(task)
}

/// (placeholder_title, title_auto). Trims input, falls back to description
/// first-line, then to `"未命名任务"`.
fn resolve_initial_title(user_title: &str, description: &str) -> (String, bool) {
    let trimmed = user_title.trim();
    if !trimmed.is_empty() {
        return (trimmed.to_string(), false);
    }
    let from_desc = description
        .lines()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .map(|l| {
            // Truncate at a char boundary, not a byte boundary — CJK input is common here.
            let mut end = l.len();
            if l.chars().count() > 60 {
                end = l
                    .char_indices()
                    .nth(60)
                    .map(|(i, _)| i)
                    .unwrap_or(l.len());
            }
            l[..end].to_string()
        });
    match from_desc {
        Some(s) => (s, true),
        None => ("未命名任务".to_string(), true),
    }
}

/// Replace a task's title. When `auto` is `false`, also clears `title_auto`
/// so future ai-title sync passes will not overwrite the new value. When
/// `auto` is `true`, only writes if `title_auto` is currently `true` — used
/// by the master-session aiTitle reconcile path to avoid clobbering a
/// user-edited title.
pub fn set_task_title(task_id: &str, new_title: &str, auto: bool) -> Result<(), String> {
    let trimmed = new_title.trim();
    if trimmed.is_empty() {
        return Err("title cannot be empty".to_string());
    }
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    if auto {
        if !task.title_auto {
            return Ok(());
        }
        if task.title == trimmed {
            return Ok(());
        }
        task.title = trimmed.to_string();
        // Keep title_auto=true so subsequent ai-title revisions still flow through.
    } else {
        task.title = trimmed.to_string();
        task.title_auto = false;
    }
    write_task_atomic(&task)
}

/// Read a single task by id.
pub fn get_task(task_id: &str) -> Result<Task, String> {
    let path = task_json_path(task_id)?;
    let s = fs::read_to_string(&path)
        .map_err(|e| format!("read task {task_id}: {e}"))?;
    serde_json::from_str(&s).map_err(|e| format!("parse task {task_id}: {e}"))
}

/// List every task, newest-first. When `project_id` is `Some`, filter to that
/// project.
pub fn list_tasks(project_id: Option<&str>) -> Vec<Task> {
    let Ok(dir) = tasks_dir() else { return Vec::new() };
    if !dir.exists() {
        return Vec::new();
    }
    let Ok(entries) = fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<Task> = entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let path = e.path();
            if path.extension().and_then(|s| s.to_str()) != Some("json") {
                return None;
            }
            let s = fs::read_to_string(&path).ok()?;
            serde_json::from_str::<Task>(&s).ok()
        })
        .filter(|t| project_id.map_or(true, |p| t.project_id == p))
        .collect();
    out.sort_by(|a, b| b.created_at.cmp(&a.created_at));
    out
}

/// Replace a task's plan. Acquires the per-task write mutex (PRD §5.7).
pub fn update_plan(task_id: &str, plan: DagPlan) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
    task.plan = plan;
    write_task_atomic(&task)
}

/// Append a file material to the task's inbox. `filename` is sanitized; the
/// stored path lives at `<materials_dir>/<safe_name>`. Returns the persisted
/// absolute path so the caller (frontend) can refer to it.
pub fn add_task_material(
    task_id: &str,
    filename: &str,
    bytes: &[u8],
    media: MediaKind,
) -> Result<PathBuf, String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let materials = task_materials_dir(task_id)?;
    fs::create_dir_all(&materials).map_err(|e| format!("mkdir materials: {e}"))?;
    let safe = sanitize_material_name(filename);
    let dest = materials.join(&safe);
    fs::write(&dest, bytes).map_err(|e| format!("write material file: {e}"))?;
    let mut task = get_task(task_id)?;
    let now = chrono::Utc::now().timestamp();
    task.inbox_materials.push(Material::File {
        path: dest.clone(),
        media,
        added_at: now,
    });
    write_task_atomic(&task)?;
    Ok(dest)
}

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
    let spec = crate::master::spawn_spec_from_task(&task)?;
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


fn slugify_title(title: &str) -> String {
    let mut slug: String = title
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-').to_string();
    let truncated: String = slug.chars().take(40).collect();
    if truncated.is_empty() {
        "task".to_string()
    } else {
        truncated
    }
}

fn sanitize_material_name(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stripped: String = cleaned
        .trim_matches(|c: char| c == '.' || c == '_' || c == '-')
        .to_string();
    if stripped.is_empty() {
        "material".to_string()
    } else {
        stripped
    }
}

fn pick_unique_branch(workspace: &Path, slug: &str) -> Result<String, String> {
    let base = format!("fleet/{slug}");
    if !git_branch_exists(workspace, &base)? {
        return Ok(base);
    }
    for i in 1..1000 {
        let candidate = format!("{base}-{i}");
        if !git_branch_exists(workspace, &candidate)? {
            return Ok(candidate);
        }
    }
    Err(format!("could not find unique branch name for {base}"))
}

fn git_branch_exists(workspace: &Path, branch: &str) -> Result<bool, String> {
    let repo = git2::Repository::open(workspace)
        .map_err(|e| format!("open git repo at {}: {}", workspace.display(), e.message()))?;
    let exists = repo.find_branch(branch, git2::BranchType::Local).is_ok();
    Ok(exists)
}

fn git_create_branch(workspace: &Path, branch: &str) -> Result<(), String> {
    let repo = git2::Repository::open(workspace)
        .map_err(|e| format!("open git repo at {}: {}", workspace.display(), e.message()))?;
    let head_commit = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| format!("read HEAD commit: {}", e.message()))?;
    repo.branch(branch, &head_commit, false)
        .map_err(|e| format!("create branch {branch}: {}", e.message()))?;
    repo.set_head(&format!("refs/heads/{branch}"))
        .map_err(|e| format!("set HEAD to {branch}: {}", e.message()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitem::{PItem, PItemStatus};
    use crate::plan::DagPlan;

    fn pitem(id: &str, status: PItemStatus) -> PItem {
        PItem {
            id: id.into(),
            desc: id.into(),
            touches: vec![],
            depends_on: vec![],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![],
            artifacts: vec![],
            skippable: None,
            human_gate: false,
            status,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        }
    }

    #[test]
    fn drafting_constructor_has_empty_plan() {
        let t = Task::drafting("t1".into(), "p1".into(), "do thing".into(), 1_700_000_000);
        assert_eq!(t.id, "t1");
        assert_eq!(t.project_id, "p1");
        assert!(matches!(t.status, TaskStatus::Drafting));
        assert!(t.plan.is_empty());
        assert!(t.task_branch.is_none());
    }

    #[test]
    fn is_plan_finished_handles_empty_and_full() {
        let mut t = Task::drafting("t1".into(), "p1".into(), "x".into(), 0);
        // Empty plan should NOT be reported as finished — there's nothing to do.
        assert!(!t.is_plan_finished());

        t.plan = DagPlan::from_items(vec![
            pitem("a", PItemStatus::Done),
            pitem("b", PItemStatus::Skipped),
        ]);
        assert!(t.is_plan_finished());

        t.plan = DagPlan::from_items(vec![
            pitem("a", PItemStatus::Done),
            pitem("b", PItemStatus::Running),
        ]);
        assert!(!t.is_plan_finished());
    }

    #[test]
    fn serde_roundtrip_task() {
        let mut t = Task::drafting("t1".into(), "p1".into(), "demo".into(), 1_700_000_000);
        t.description = "details".into();
        t.task_branch = Some("fleet/demo".into());
        t.inbox_materials = vec![
            Material::File {
                path: PathBuf::from("/abs/path/a.png"),
                media: MediaKind::Screenshot,
                added_at: 1_700_000_001,
            },
            Material::Text {
                content: "hello".into(),
                added_at: 1_700_000_002,
            },
        ];
        t.plan = DagPlan::from_items(vec![pitem("p1", PItemStatus::WaitDeps)]);
        let json = serde_json::to_string(&t).unwrap();
        let back: Task = serde_json::from_str(&json).unwrap();
        assert_eq!(t, back);
    }

    #[test]
    fn slugify_title_handles_punctuation_and_unicode() {
        assert_eq!(slugify_title("Add user settings page"), "add-user-settings-page");
        assert_eq!(slugify_title("  Add  user!!  settings"), "add-user-settings");
        assert_eq!(slugify_title("V2 — refactor / migrate"), "v2-refactor-migrate");
        assert_eq!(slugify_title("!!!"), "task");
        assert_eq!(slugify_title(""), "task");
    }

    #[test]
    fn slugify_title_truncates_long_inputs() {
        let title = "a".repeat(80);
        let slug = slugify_title(&title);
        assert!(slug.len() <= 40);
        assert!(slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'));
    }

    #[test]
    fn sanitize_material_name_strips_path_traversal() {
        assert_eq!(sanitize_material_name("foo.png"), "foo.png");
        assert_eq!(sanitize_material_name("../etc/passwd"), "etc_passwd");
        assert_eq!(sanitize_material_name("../"), "material");
        assert_eq!(sanitize_material_name("clean-name_v2.txt"), "clean-name_v2.txt");
        assert_eq!(sanitize_material_name("/ ?"), "material");
    }

    #[test]
    fn task_input_roundtrip() {
        let input = TaskInput {
            project_id: "p1".into(),
            title: "demo".into(),
            description: "details".into(),
        };
        let json = serde_json::to_string(&input).unwrap();
        let back: TaskInput = serde_json::from_str(&json).unwrap();
        assert_eq!(back.project_id, "p1");
        assert_eq!(back.title, "demo");
    }

    #[test]
    fn task_event_roundtrip_all_variants() {
        let events = vec![
            TaskEvent::TaskStatusChanged {
                task_id: "t1".into(),
                status: TaskStatus::Running,
            },
            TaskEvent::PItemStatusChanged {
                task_id: "t1".into(),
                p_item_id: "p1".into(),
                status: PItemStatus::Running,
            },
            TaskEvent::PlanUpdated { task_id: "t1".into() },
            TaskEvent::MaterialAdded { task_id: "t1".into() },
            TaskEvent::MasterMessage {
                task_id: "t1".into(),
                source: MasterMessageSource::Event,
                message: "P1 worker done".into(),
            },
        ];
        for ev in events {
            let json = serde_json::to_string(&ev).unwrap();
            let back: TaskEvent = serde_json::from_str(&json).unwrap();
            assert_eq!(ev, back);
        }
    }

    #[test]
    fn resolve_initial_title_prefers_user_title() {
        let (title, auto) = resolve_initial_title("My task", "ignored");
        assert_eq!(title, "My task");
        assert!(!auto);
    }

    #[test]
    fn resolve_initial_title_falls_back_to_description_first_line() {
        let (title, auto) = resolve_initial_title(
            "",
            "实现书签 UI\n详细描述\n第三行",
        );
        assert_eq!(title, "实现书签 UI");
        assert!(auto);
    }

    #[test]
    fn resolve_initial_title_truncates_long_description_at_char_boundary() {
        let desc = "中".repeat(120);
        let (title, auto) = resolve_initial_title("", &desc);
        assert_eq!(title.chars().count(), 60);
        assert!(title.is_char_boundary(title.len()));
        assert!(auto);
    }

    #[test]
    fn resolve_initial_title_defaults_when_all_blank() {
        let (title, auto) = resolve_initial_title("   ", "\n\n   ");
        assert_eq!(title, "未命名任务");
        assert!(auto);
    }

    #[test]
    fn deserialize_minimal_task_json() {
        // Older snapshots may predate optional fields; serde(default) fills them.
        let json = r#"{
            "id": "t1",
            "projectId": "p1",
            "title": "demo",
            "status": "ready",
            "createdAt": 1700000000
        }"#;
        let t: Task = serde_json::from_str(json).unwrap();
        assert_eq!(t.id, "t1");
        assert!(matches!(t.status, TaskStatus::Ready));
        assert!(t.plan.is_empty());
        assert!(t.inbox_materials.is_empty());
    }

    fn init_repo_with_first_commit(path: &Path) {
        let repo = git2::Repository::init(path).expect("init repo");
        let sig = git2::Signature::now("Test", "test@example.com").expect("sig");
        let tree_oid = {
            let tb = repo.treebuilder(None).expect("treebuilder");
            tb.write().expect("tree write")
        };
        let tree = repo.find_tree(tree_oid).expect("find tree");
        repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
            .expect("initial commit");
    }

    #[test]
    fn git_branch_exists_returns_false_for_missing_branch() {
        let tmp = tempfile::TempDir::new().unwrap();
        init_repo_with_first_commit(tmp.path());
        let exists = git_branch_exists(tmp.path(), "nonexistent").unwrap();
        assert!(!exists);
    }

    #[test]
    fn git_create_branch_creates_and_moves_head() {
        let tmp = tempfile::TempDir::new().unwrap();
        init_repo_with_first_commit(tmp.path());
        git_create_branch(tmp.path(), "fleet/demo").unwrap();
        assert!(git_branch_exists(tmp.path(), "fleet/demo").unwrap());
        let repo = git2::Repository::open(tmp.path()).unwrap();
        let head = repo.head().unwrap();
        assert_eq!(head.shorthand(), Some("fleet/demo"));
    }

    #[test]
    fn git_create_branch_errors_on_duplicate() {
        let tmp = tempfile::TempDir::new().unwrap();
        init_repo_with_first_commit(tmp.path());
        git_create_branch(tmp.path(), "fleet/dup").unwrap();
        let second = git_create_branch(tmp.path(), "fleet/dup");
        assert!(second.is_err(), "duplicate branch should error");
    }

    #[test]
    fn git_branch_exists_errors_when_not_a_repo() {
        let tmp = tempfile::TempDir::new().unwrap();
        let result = git_branch_exists(tmp.path(), "any");
        assert!(result.is_err());
    }
}
