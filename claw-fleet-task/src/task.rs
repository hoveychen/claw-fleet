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
use crate::paths::get_fleet_dir;

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
    /// Phase 3: workspace path persisted at `start_task` time so out-of-process
    /// tools (fleet-cli, master tool calls) can resolve the working tree
    /// without consulting the project table. Legacy tasks (pre-Phase-3) leave
    /// this `None` and fall back to the project lookup.
    #[serde(default)]
    pub workspace: Option<PathBuf>,
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
    /// Optional per-task model override picked in the composer. When set, the
    /// planning/master session runs on this model instead of the default
    /// `planning::PLANNER_MODEL`. `None` (and legacy task JSONs) fall back to
    /// the default. Workers/review keep their own defaults.
    #[serde(default)]
    pub model: Option<String>,
    /// Result of the task-level end-to-end verification pass (P4). `None` until
    /// the orchestrator runs it (after every P-item is terminal, before
    /// `AwaitingAcceptance`). A failed run keeps the task OUT of
    /// `AwaitingAcceptance` so a broken integration can't be silently accepted.
    #[serde(default)]
    pub e2e: Option<E2eOutcome>,
    /// The branch HEAD pointed at when `start_task` forked the task branch —
    /// recorded BEFORE `git_create_branch` switches HEAD to `fleet/<slug>`, so
    /// `accept_task`'s merge-back knows which branch to merge into. `None` for
    /// legacy tasks (pre-this-field) and tasks not yet started; the merge-back
    /// path then falls back to a main/master heuristic.
    #[serde(default)]
    pub base_branch: Option<String>,
}

/// Outcome of the task-level e2e command configured in `fleet.yaml`'s
/// `verify.e2e`. Recorded on the `Task` so the UI / user can see why a finished
/// plan didn't reach acceptance.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct E2eOutcome {
    /// The command that was run (from `verify.e2e`).
    pub command: String,
    /// `true` when the command exited 0.
    pub passed: bool,
    /// On failure, the command line + tail of its output.
    #[serde(default)]
    pub gaps: Vec<String>,
    /// Unix timestamp of the run.
    pub ran_at: i64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TaskStatus {
    /// User is still drafting in Inbox, or the planning session is clarifying
    /// requirements and building the DAG. No `start_task` yet.
    Drafting,
    /// Orchestrator is dispatching workers / reviews per the dependency graph.
    Running,
    /// `pause` issued — orchestrator + sessions SIGSTOPped.
    Paused,
    /// All P-items terminal (Done / Skipped / Failed); a final review pass is
    /// judging the task as a whole before it goes to the user.
    Reviewing,
    /// Review passed; the task is parked waiting for the user's final
    /// acceptance (see P7). The user confirms → `Done`.
    AwaitingAcceptance,
    /// User accepted the completed task.
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
    /// Optional per-task model override (composer model selector). Threaded
    /// onto the created `Task` and used for the planning/master session.
    #[serde(default)]
    pub model: Option<String>,
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
            workspace: None,
            master_session_id: None,
            title_auto: false,
            model: None,
            e2e: None,
            base_branch: None,
        }
    }

    /// Current checked-out branch shorthand of `workspace` (e.g. `main`), or
    /// `None` when detached / unreadable. Recorded as `base_branch` at
    /// `start_task` so the eventual merge-back targets the right branch.
    pub fn current_branch(workspace: &Path) -> Option<String> {
        let repo = git2::Repository::open(workspace).ok()?;
        let head = repo.head().ok()?;
        head.shorthand().map(|s| s.to_string())
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

pub fn task_write_lock(id: &str) -> Arc<Mutex<()>> {
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

pub fn write_task_atomic(task: &Task) -> Result<(), String> {
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
    task.model = input.model.filter(|m| !m.trim().is_empty());
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


pub fn slugify_title(title: &str) -> String {
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

pub fn pick_unique_branch(workspace: &Path, slug: &str) -> Result<String, String> {
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

pub fn git_branch_exists(workspace: &Path, branch: &str) -> Result<bool, String> {
    let repo = git2::Repository::open(workspace)
        .map_err(|e| format!("open git repo at {}: {}", workspace.display(), e.message()))?;
    let exists = repo.find_branch(branch, git2::BranchType::Local).is_ok();
    Ok(exists)
}

pub fn git_create_branch(workspace: &Path, branch: &str) -> Result<(), String> {
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
            acceptance: vec![],
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
            model: None,
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
            "status": "running",
            "createdAt": 1700000000
        }"#;
        let t: Task = serde_json::from_str(json).unwrap();
        assert_eq!(t.id, "t1");
        assert!(matches!(t.status, TaskStatus::Running));
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
    fn current_branch_reports_checked_out_branch() {
        let tmp = tempfile::TempDir::new().unwrap();
        init_repo_with_first_commit(tmp.path());
        // Fresh repo's default branch (master or main depending on git config).
        let b = Task::current_branch(tmp.path()).expect("should read a branch");
        assert!(!b.is_empty());
        // After creating + switching to a task branch, current_branch tracks it.
        git_create_branch(tmp.path(), "fleet/demo").unwrap();
        assert_eq!(Task::current_branch(tmp.path()).as_deref(), Some("fleet/demo"));
    }

    #[test]
    fn current_branch_none_when_not_a_repo() {
        let tmp = tempfile::TempDir::new().unwrap();
        assert!(Task::current_branch(tmp.path()).is_none());
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
