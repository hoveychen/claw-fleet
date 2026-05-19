//! Master spawn-spec builder. Pure data — the actual subprocess
//! lifecycle (Claude Code spawn, SIGSTOP, destroy) lives in
//! `claw_fleet_core::supervisor` / `claw-fleet-desktop::local_backend`.
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
//!   (PRD §5.7 — "Master cwd = task 主 branch working tree"). The caller
//!   supplies this path explicitly — this crate has no knowledge of Project.
//! - **SYSTEM prompt**: rendered by `system_template::compose_system_prompt`.

use std::path::PathBuf;

use crate::master::system_template::compose_system_prompt;
use crate::task::{Task, TaskId};

/// Everything a caller needs to actually launch a master session. Returned
/// by `spawn_spec_from_task` and consumed by the supervisor's spawn layer.
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

/// Build a `MasterSpawnSpec` for a loaded `Task`, given the project's
/// workspace path (the caller resolves Project → workspace, since this
/// crate doesn't know about Project). Errors when:
/// - the workspace path is missing or not a directory
pub fn spawn_spec_from_task(task: &Task, workspace: PathBuf) -> Result<MasterSpawnSpec, String> {
    if !workspace.is_dir() {
        return Err(format!(
            "project workspace does not exist or is not a directory: {}",
            workspace.display()
        ));
    }
    let system_prompt = compose_system_prompt(task);
    Ok(MasterSpawnSpec {
        task_id: task.id.clone(),
        cwd: workspace,
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
    use crate::task::{create_task, MediaKind, TaskInput, TaskStatus};
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
        let _g = crate::paths::fleet_home_lock();
        let home = TempDir::new().unwrap();
        let ws = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());

        let task = create_task(TaskInput {
            project_id: "demo".into(),
            title: "demo".into(),
            description: "do it".into(),
        })
        .unwrap();

        let spec = spawn_spec_from_task(&task, ws.path().to_path_buf()).unwrap();
        assert_eq!(spec.task_id, task.id);
        assert_eq!(spec.cwd, ws.path());
        assert_eq!(spec.model, MASTER_MODEL);
        assert!(spec.system_prompt.contains(&task.id));
        assert!(spec.system_prompt.contains("demo"));
        assert!(spec.system_prompt.contains("Acceptance Audit Protocol"));
    }

    #[test]
    fn spawn_spec_errors_when_workspace_missing() {
        let _g = crate::paths::fleet_home_lock();
        let home = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());

        // Workspace points at an absolute path guaranteed not to exist.
        let ghost_path = {
            let td = TempDir::new().unwrap();
            td.path().to_path_buf()
        };
        let task = create_task(TaskInput {
            project_id: "ghost".into(),
            title: "x".into(),
            description: String::new(),
        })
        .unwrap();

        let err = spawn_spec_from_task(&task, ghost_path).unwrap_err();
        assert!(err.contains("workspace does not exist"));
    }

    #[test]
    fn spawn_spec_includes_plan_for_running_task() {
        let _g = crate::paths::fleet_home_lock();
        let home = TempDir::new().unwrap();
        let ws = TempDir::new().unwrap();
        let _override = FleetHomeOverride::new(home.path());

        let mut task = create_task(TaskInput {
            project_id: "demo".into(),
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
        let _ = (MediaKind::Document, TaskStatus::Drafting);

        let spec = spawn_spec_from_task(&task, ws.path().to_path_buf()).unwrap();
        assert!(spec.system_prompt.contains("feat-x"));
        assert!(spec.system_prompt.contains("total=1"));
    }
}
