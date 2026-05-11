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

use claw_fleet_core::pitem::{PItem, PItemStatus};
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
    })
    .expect("create_project");

    // create_task: file appears on disk.
    let task = task::create_task(TaskInput {
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
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![],
            artifacts: vec![],
            skippable: None,
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
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![],
            artifacts: vec![],
            skippable: None,
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
fn master_mutations_flip_p_item_statuses_correctly() {
    let _g = FLEET_HOME_LOCK.lock().unwrap();
    let home = fresh_tmp("master-muts");
    let _override = FleetHomeOverride::new(&home);

    let workspace = fresh_tmp("master-muts-ws");
    git_init_repo(&workspace);
    let proj = project::create_project(ProjectInput {
        name: "demo".into(),
        workspace: workspace.to_string_lossy().to_string(),
        concurrency: Some(1),
    })
    .unwrap();
    let t = task::create_task(TaskInput {
        project_id: proj.id,
        title: "demo".into(),
        description: String::new(),
    })
    .unwrap();
    let plan = DagPlan::from_items(vec![
        PItem {
            id: "p1".into(),
            desc: "first".into(),
            touches: vec![],
            depends_on: vec![],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![],
            artifacts: vec![],
            skippable: None,
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
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![],
            artifacts: vec![],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        },
    ]);
    task::update_plan(&t.id, plan).unwrap();

    // dispatch_pitem flips WaitDeps → Running
    task::dispatch_pitem(&t.id, "p1").unwrap();
    let after_dispatch = task::get_task(&t.id).unwrap();
    assert!(matches!(after_dispatch.plan.get("p1").unwrap().status, PItemStatus::Running));
    assert!(after_dispatch.plan.get("p1").unwrap().started_at.is_some());

    // mark_done flips → Done with summary
    task::mark_done(&t.id, "p1", "OK shipped").unwrap();
    let after_done = task::get_task(&t.id).unwrap();
    assert!(matches!(after_done.plan.get("p1").unwrap().status, PItemStatus::Done));
    assert_eq!(
        after_done.plan.get("p1").unwrap().output_summary.as_deref(),
        Some("OK shipped")
    );

    // dispatch_pitem on Done item errors
    assert!(task::dispatch_pitem(&t.id, "p1").is_err());

    // mark_failed on p2 → Failed, propagate_skip empty (no downstream)
    let skipped = task::mark_failed(&t.id, "p2", claw_fleet_core::pitem::FailReason::BuildFailed)
        .unwrap();
    assert!(skipped.is_empty());
    let after_fail = task::get_task(&t.id).unwrap();
    assert!(matches!(
        after_fail.plan.get("p2").unwrap().status,
        PItemStatus::Failed(_)
    ));
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
    })
    .unwrap();
    let t = task::create_task(TaskInput {
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
    })
    .unwrap();

    // Pre-create `fleet/demo-task` so start_task must fall back to `-1`.
    let pre_status = Command::new("git")
        .args(["-C", workspace.to_str().unwrap(), "branch", "fleet/demo-task"])
        .status()
        .unwrap();
    assert!(pre_status.success(), "pre-create branch");

    let t = task::create_task(TaskInput {
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
