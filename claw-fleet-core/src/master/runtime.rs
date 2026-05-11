//! Master spawn-spec builder. Pure data — the actual subprocess
//! lifecycle (Claude Code spawn, SIGSTOP, destroy) lives in
//! `crate::supervisor` / `claw-fleet-desktop::local_backend`.
//!
//! Why pure: the spawn spec is testable from any thread without root or
//! Claude CLI installed, and the supervisor can mock different specs in
//! its own tests (e.g. inject a fake master for the worker-executor tests
//! that come later).
//!
//! Variables this layer fixes:
//! - **Model**: `claude-sonnet-4-6` (PRD §5.7 — Sonnet 4.6 by default; user
//!   can override via Fleet's existing model-selection settings later).
//! - **cwd**: the project workspace root on the task's `fleet/<slug>` branch
//!   (PRD §5.7 — "Master cwd = task 主 branch working tree").
//! - **SYSTEM prompt**: rendered by `system_template::compose_system_prompt`.

use std::path::PathBuf;

use crate::master::system_template::compose_system_prompt;
use crate::project::list_projects;
use crate::task::{get_task, Task, TaskId};

/// Everything a caller needs to actually launch a master session. Returned
/// by `spawn_spec_for_task` and consumed by the supervisor's spawn layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterSpawnSpec {
    pub task_id: TaskId,
    /// The project's working tree on the task's `fleet/<slug>` branch.
    pub cwd: PathBuf,
    /// Composed master SYSTEM prompt (see system_template.rs).
    pub system_prompt: String,
    /// Model id passed to Claude Code. Anthropic API uses
    /// `claude-sonnet-4-6` for Sonnet 4.6 (per PRD §5.7).
    pub model: &'static str,
}

/// Default master model. Centralised here so changing it doesn't require a
/// hunt through code paths.
pub const MASTER_MODEL: &str = "claude-sonnet-4-6";

/// Build a `MasterSpawnSpec` for `task_id`. Errors when:
/// - the task doesn't exist on disk
/// - the task has no project_id reachable (orphaned)
/// - the project has no `workspace` path on disk
pub fn spawn_spec_for_task(task_id: &str) -> Result<MasterSpawnSpec, String> {
    let task = get_task(task_id)?;
    spawn_spec_from_task(&task)
}

/// Same as `spawn_spec_for_task` but for callers that already loaded the
/// `Task` (e.g. supervisor reads it while transitioning to Running).
pub fn spawn_spec_from_task(task: &Task) -> Result<MasterSpawnSpec, String> {
    let project = list_projects()
        .into_iter()
        .find(|p| p.id == task.project_id)
        .ok_or_else(|| format!("project {} not found for task {}", task.project_id, task.id))?;
    let cwd = PathBuf::from(&project.workspace);
    if !cwd.is_dir() {
        return Err(format!(
            "project workspace does not exist or is not a directory: {}",
            cwd.display()
        ));
    }
    let system_prompt = compose_system_prompt(task);
    Ok(MasterSpawnSpec {
        task_id: task.id.clone(),
        cwd,
        system_prompt,
        model: MASTER_MODEL,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitem::PItem;
    use crate::pitem::PItemStatus;
    use crate::plan::DagPlan;
    use crate::project::{create_project, ProjectInput};
    use crate::task::{create_task, MediaKind, Task, TaskInput, TaskStatus};
    use std::fs;
    use tempfile::TempDir;

    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
    }

    impl FleetHomeOverride {
        fn new(tmp: &std::path::Path) -> Self {
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

    #[test]
    fn spawn_spec_carries_task_cwd_and_filled_prompt() {
        let _g = crate::session::fleet_home_lock();
        let home = TempDir::new().unwrap();
        let ws = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());

        let proj = create_project(ProjectInput {
            name: "demo".into(),
            workspace: ws.path().to_string_lossy().to_string(),
            concurrency: Some(1),
        })
        .unwrap();
        let task = create_task(TaskInput {
            project_id: proj.id.clone(),
            title: "demo".into(),
            description: "do it".into(),
        })
        .unwrap();

        let spec = spawn_spec_for_task(&task.id).unwrap();
        assert_eq!(spec.task_id, task.id);
        assert_eq!(spec.cwd, ws.path());
        assert_eq!(spec.model, MASTER_MODEL);
        assert!(spec.system_prompt.contains(&task.id));
        assert!(spec.system_prompt.contains("demo"));
        assert!(spec.system_prompt.contains("Acceptance Audit Protocol"));
    }

    #[test]
    fn spawn_spec_errors_when_project_workspace_missing() {
        let _g = crate::session::fleet_home_lock();
        let home = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());

        // Project workspace points at a nonexistent directory.
        let proj = create_project(ProjectInput {
            name: "ghost".into(),
            workspace: "/tmp/definitely-not-here-{}".into(),
            concurrency: Some(1),
        })
        .unwrap();
        let task = create_task(TaskInput {
            project_id: proj.id,
            title: "x".into(),
            description: String::new(),
        })
        .unwrap();

        let err = spawn_spec_for_task(&task.id).unwrap_err();
        assert!(err.contains("workspace does not exist"));
    }

    #[test]
    fn spawn_spec_errors_when_project_orphaned() {
        let _g = crate::session::fleet_home_lock();
        let home = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());

        // Hand-roll a task json that points at a project id no one created.
        let task = Task {
            id: "orphan".into(),
            project_id: "nope".into(),
            title: "x".into(),
            description: String::new(),
            inbox_materials: vec![],
            plan: DagPlan::default(),
            status: TaskStatus::Drafting,
            created_at: 0,
            started_at: None,
            completed_at: None,
            task_branch: None,
            master_session_id: None,
        };
        let path = crate::task::task_json_path(&task.id).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, serde_json::to_string(&task).unwrap()).unwrap();

        let err = spawn_spec_for_task(&task.id).unwrap_err();
        assert!(err.contains("project nope not found"));
    }

    #[test]
    fn spawn_spec_includes_plan_for_running_task() {
        let _g = crate::session::fleet_home_lock();
        let home = TempDir::new().unwrap();
        let ws = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());
        let proj = create_project(ProjectInput {
            name: "demo".into(),
            workspace: ws.path().to_string_lossy().to_string(),
            concurrency: Some(1),
        })
        .unwrap();
        let mut task = create_task(TaskInput {
            project_id: proj.id,
            title: "t".into(),
            description: String::new(),
        })
        .unwrap();
        task.plan = DagPlan::from_items(vec![PItem {
            id: "feat-x".into(),
            desc: "build feat x".into(),
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
        }]);
        crate::task::update_plan(&task.id, task.plan.clone()).unwrap();
        // Suppress unused-var warning for materials placeholder. MediaKind
        // is referenced so we keep its import meaningful.
        let _ = MediaKind::Document;

        let spec = spawn_spec_for_task(&task.id).unwrap();
        assert!(spec.system_prompt.contains("feat-x"));
        assert!(spec.system_prompt.contains("total=1"));
    }
}
