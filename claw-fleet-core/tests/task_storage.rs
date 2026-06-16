//! Integration test for the task storage layer (P3 acceptance).
//!
//! Sets `FLEET_HOME` to a temp dir so `~/.fleet/tasks/` resolves there, and
//! initializes a temp git repo so `start_task` has a real working tree to
//! create the `fleet/<slug>` branch in.
//!
//! All tests in this file serialize on a local `FLEET_HOME_LOCK` because
//! `real_home_dir()` reads the global env. The same pattern is used in
//! `claw-fleet-core/src/supervisor.rs::tests`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use claw_fleet_core::pitem::{AcceptanceCriterion, PItem, PItemStatus};
use claw_fleet_core::plan::DagPlan;
use claw_fleet_core::project::{self, ProjectInput};
use claw_fleet_core::task::{
    self, MediaKind, Task, TaskInput, TaskStatus,
};

static FLEET_HOME_LOCK: Mutex<()> = Mutex::new(());

struct FleetHomeOverride {
    prev: Option<std::ffi::OsString>,
}

impl FleetHomeOverride {
    fn new(tmp: &Path) -> Self {
        let prev = std::env::var_os("FLEET_HOME");
        unsafe { std::env::set_var("FLEET_HOME", tmp) };
        FleetHomeOverride { prev }
    }
}

impl Drop for FleetHomeOverride {
    fn drop(&mut self) {
        unsafe {
            match &self.prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
    }
}

fn fresh_tmp(tag: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "fleet-task-test-{}-{}-{}",
        tag,
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn git_init_repo(path: &Path) {
    // Best-effort: init + initial commit so we have a `main` ref and can
    // checkout new branches off it.
    Command::new("git")
        .args(["init", "-q", "-b", "main"])
        .current_dir(path)
        .status()
        .expect("git init");
    // Don't pollute user identity; set per-repo.
    Command::new("git")
        .args(["config", "user.email", "fleet@example.com"])
        .current_dir(path)
        .status()
        .expect("git config email");
    Command::new("git")
        .args(["config", "user.name", "Fleet Test"])
        .current_dir(path)
        .status()
        .expect("git config name");
    std::fs::write(path.join("README.md"), "test\n").unwrap();
    Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .status()
        .expect("git add");
    Command::new("git")
        .args(["commit", "-q", "-m", "init"])
        .current_dir(path)
        .status()
        .expect("git commit");
}

fn current_branch(repo: &Path) -> String {
    let out = Command::new("git")
        .args(["rev-parse", "--abbrev-ref", "HEAD"])
        .current_dir(repo)
        .output()
        .expect("git rev-parse");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

#[test]
fn full_lifecycle_create_update_material_start() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("lifecycle");
    let _override = FleetHomeOverride::new(&home);

    // Create a project pointing at a fresh git workspace.
    let workspace = fresh_tmp("lifecycle-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .expect("create_project");

    // create_task: file appears on disk.
    let task = task::create_task(TaskInput {
            model: None,
        project_id: proj.id.clone(),
        title: "Add Bookmarks UI".into(),
        description: "see Inbox materials".into(),
    })
    .expect("create_task");
    let json_path = task::task_json_path(&task.id).unwrap();
    assert!(json_path.exists(), "task json should exist at {json_path:?}");
    let materials_dir = task::task_materials_dir(&task.id).unwrap();
    assert!(materials_dir.exists(), "materials dir should be created");

    // get_task round-trips.
    let read_back = task::get_task(&task.id).expect("get_task");
    assert_eq!(read_back.id, task.id);
    assert_eq!(read_back.title, "Add Bookmarks UI");
    assert!(matches!(read_back.status, TaskStatus::Drafting));

    // list_tasks returns it; project filter works.
    let listed = task::list_tasks(Some(&proj.id));
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, task.id);
    assert!(task::list_tasks(Some("nonexistent")).is_empty());

    // add_task_material: file appears under materials/, task json references path.
    let bytes = b"hello\n";
    let stored = task::add_task_material(&task.id, "screenshot.png", bytes, MediaKind::Screenshot)
        .expect("add_task_material");
    assert!(stored.exists(), "material file should be persisted");
    assert!(
        stored.starts_with(&materials_dir),
        "material must live under materials dir: {stored:?}"
    );
    let after_material = task::get_task(&task.id).unwrap();
    assert_eq!(after_material.inbox_materials.len(), 1);

    // update_plan: persisted across get_task.
    let plan = DagPlan::from_items(vec![
        PItem {
            id: "p1".into(),
            desc: "first".into(),
            touches: vec![],
            depends_on: vec![],
            acceptance: vec![AcceptanceCriterion::Custom("true".into())],
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        },
        PItem {
            id: "p2".into(),
            desc: "second".into(),
            touches: vec![],
            depends_on: vec!["p1".into()],
            acceptance: vec![AcceptanceCriterion::Custom("true".into())],
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        },
    ]);
    task::update_plan(&task.id, plan.clone()).expect("update_plan");
    let after_plan = task::get_task(&task.id).unwrap();
    assert_eq!(after_plan.plan.items.len(), 2);
    assert!(after_plan.plan.items.contains_key("p1"));

    // start_task: branch is auto-created + task_branch set + status flips to Running.
    task::start_task(&task.id).expect("start_task");
    let after_start: Task = task::get_task(&task.id).unwrap();
    assert!(matches!(after_start.status, TaskStatus::Running));
    let branch = after_start.task_branch.as_ref().expect("task_branch set");
    assert!(branch.starts_with("fleet/"), "branch should be fleet/<slug>, got {branch}");
    assert_eq!(current_branch(&workspace), *branch);
    assert!(after_start.started_at.is_some());

    // Idempotency: a second start_task does not error.
    task::start_task(&task.id).expect("start_task idempotent");
}

#[test]
fn pause_resume_clear_full_cycle() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("pause-cycle");
    let _override = FleetHomeOverride::new(&home);
    let workspace = fresh_tmp("pause-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
            model: None,
        project_id: proj.id,
        title: "demo".into(),
        description: String::new(),
    })
    .unwrap();
    task::start_task(&t.id).unwrap();

    // Running → Paused
    task::pause_task(&t.id).unwrap();
    assert!(matches!(task::get_task(&t.id).unwrap().status, TaskStatus::Paused));

    // Pause on Paused is idempotent
    task::pause_task(&t.id).unwrap();

    // Paused → Running
    task::resume_task(&t.id).unwrap();
    assert!(matches!(task::get_task(&t.id).unwrap().status, TaskStatus::Running));

    // Clear removes the json + materials dir
    task::clear_task(&t.id).unwrap();
    assert!(task::get_task(&t.id).is_err());
}

// `start_task_enqueues_master_session_*` was deleted in the task-subsystem
// rebuild: start_task no longer spawns an LLM master. It now only preps the
// task (branch + Running); the deterministic orchestrator dispatches workers.
// Branch prep is covered by `start_task_picks_unique_branch_when_slug_collides`.

#[test]
fn start_task_preps_branch_and_running_without_master() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("start-prep");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("start-prep-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
        manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
            model: None,
        project_id: proj.id,
        title: "prep check".into(),
        description: "".into(),
    })
    .unwrap();

    task::start_task(&t.id).expect("start_task");
    let after = task::get_task(&t.id).unwrap();
    // Prepped for the orchestrator: Running + branch, NO master session.
    assert!(matches!(after.status, TaskStatus::Running));
    assert!(after.task_branch.is_some());
    assert!(after.master_session_id.is_none(), "no master is spawned anymore");
}

#[test]
fn dispatch_pitem_enqueues_worker_session_and_records_agent_session_id() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("worker-spawn");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("worker-spawn-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
            model: None,
        project_id: proj.id,
        title: "worker spawn check".into(),
        description: "".into(),
    })
    .unwrap();
    let plan = DagPlan::from_items(vec![PItem {
        id: "p1".into(),
        desc: "wire it up".into(),
        touches: vec![],
        depends_on: vec![],
        acceptance: vec![AcceptanceCriterion::Custom("true".into())],
        human_gate: false,
        status: PItemStatus::WaitDeps,
        agent_session_id: None,
        started_at: None,
        completed_at: None,
        output_summary: None,
    }]);
    task::update_plan(&t.id, plan).unwrap();

    // V2 worktree: dispatch_pitem requires a task_branch, set up by start_task.
    let _ = task::start_task(&t.id); // master enqueue may error in test env; branch + Running is what we need
    task::dispatch_pitem(&t.id, "p1").unwrap();
    let after = task::get_task(&t.id).unwrap();
    let worker_sid = after
        .plan
        .get("p1")
        .unwrap()
        .agent_session_id
        .as_ref()
        .expect("agent_session_id must be set after dispatch");

    let sessions = project::list_fleet_sessions();
    let s = sessions
        .iter()
        .find(|s| s.id == *worker_sid)
        .expect("worker session persisted");
    assert!(matches!(s.session_kind, project::SessionKind::Worker));
    assert_eq!(s.task_id.as_deref(), Some(t.id.as_str()));
    assert_eq!(s.p_item_id.as_deref(), Some("p1"));
    assert!(s.system_prompt.is_some());
    assert_eq!(s.model.as_deref(), Some("claude-sonnet-4-6"));
}

#[test]
fn clear_task_terminates_linked_sessions_and_marks_complete() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("clear-terminate");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("clear-terminate-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();
    let t = task::create_task(TaskInput {
            model: None,
        project_id: proj.id,
        title: "terminate test".into(),
        description: "".into(),
    })
    .unwrap();
    // Link a session to the task by dispatching a worker (no master spawn now).
    let plan = DagPlan::from_items(vec![PItem {
        id: "p1".into(),
        desc: "x".into(),
        touches: vec![],
        depends_on: vec![],
        acceptance: vec![AcceptanceCriterion::Custom("true".into())],
        human_gate: false,
        status: PItemStatus::WaitDeps,
        agent_session_id: None,
        started_at: None,
        completed_at: None,
        output_summary: None,
    }]);
    task::update_plan(&t.id, plan).unwrap();
    task::start_task(&t.id).expect("start_task");
    task::dispatch_pitem(&t.id, "p1").unwrap();
    let worker_sid = task::get_task(&t.id)
        .unwrap()
        .plan
        .get("p1")
        .unwrap()
        .agent_session_id
        .clone()
        .unwrap();

    task::clear_task(&t.id).expect("clear_task");

    // task json gone.
    assert!(task::get_task(&t.id).is_err());
    // The linked worker FleetSession was marked complete with a task-cleared note.
    let sessions = project::list_fleet_sessions();
    let s = sessions.iter().find(|s| s.id == worker_sid).expect("session still recorded");
    assert_eq!(s.status, project::DEFAULT_COLUMN_COMPLETE);
    assert!(s.note.as_deref().unwrap_or("").contains("task cleared"));
    assert!(s.pid.is_none());
}

// `reconcile_flips_task_to_done_when_master_exited` was deleted in the
// task-subsystem rebuild: there is no master session to key reconciliation on
// anymore. The deterministic orchestrator settles a finished plan itself
// (`OrchestratorStep::PlanComplete` → AwaitingAcceptance); P7 reworks the
// core reconcile path to that terminal state. `reconcile_task_completion`
// short-circuits (no `master_session_id`) and is a no-op in the meantime.

#[test]
fn start_task_picks_unique_branch_when_slug_collides() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("dedupe");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("dedupe-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
            manual_review_all: None,
    })
    .unwrap();

    // Pre-create `fleet/demo-task` so start_task must fall back to `-1`.
    let pre_status = Command::new("git")
        .args(["-C", workspace.to_str().unwrap(), "branch", "fleet/demo-task"])
        .status()
        .unwrap();
    assert!(pre_status.success(), "pre-create branch");

    let t = task::create_task(TaskInput {
            model: None,
        project_id: proj.id,
        title: "demo task".into(),
        description: String::new(),
    })
    .unwrap();
    task::start_task(&t.id).expect("start_task");
    let after = task::get_task(&t.id).unwrap();
    let branch = after.task_branch.unwrap();
    assert_eq!(branch, "fleet/demo-task-1");
}
