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
pub fn create_task(input: TaskInput) -> Result<Task, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let now = chrono::Utc::now().timestamp();
    let mut task = Task::drafting(id, input.project_id, input.title, now);
    task.description = input.description;
    fs::create_dir_all(task_materials_dir(&task.id)?).map_err(|e| format!("mkdir materials: {e}"))?;
    write_task_atomic(&task)?;
    Ok(task)
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
pub fn mark_done(task_id: &str, p_item_id: &str, summary: &str) -> Result<(), String> {
    let lock = task_write_lock(task_id);
    let _g = lock.lock().expect("task write mutex poisoned");
    let mut task = get_task(task_id)?;
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
    write_task_atomic(&task)
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
    // Build worker spawn spec from the task + project workspace.
    let project = crate::project::list_projects()
        .into_iter()
        .find(|p| p.id == task.project_id)
        .ok_or_else(|| format!("project {} not found for task {task_id}", task.project_id))?;
    let cwd = PathBuf::from(&project.workspace);
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
    let status = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .status()
        .map_err(|e| format!("git show-ref: {e}"))?;
    Ok(status.success())
}

fn git_create_branch(workspace: &Path, branch: &str) -> Result<(), String> {
    let out = std::process::Command::new("git")
        .arg("-C")
        .arg(workspace)
        .args(["checkout", "-b", branch])
        .output()
        .map_err(|e| format!("git checkout -b: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git checkout -b {branch} failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
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
}
