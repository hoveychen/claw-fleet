//! Spawn specs for the planning and review sessions, mirroring
//! [`crate::worker::WorkerSpawnSpec`]. The deterministic orchestrator builds
//! these and hands them to the host's launcher (`fleet-task` /
//! `ProcessLauncher`). P5 fills in the planner-prompt builder; P6 fills in the
//! review-prompt builder + judging — this module only owns the data shapes so
//! the spawn layer (P3) can be written against them.

use std::path::PathBuf;

/// A planning session: interactive (NOT safe-mode), keeps AskUserQuestion /
/// fleet__ask + preview, persists across the user's multi-turn dialogue. It
/// inherits the user's full config (principles / memory / hooks) by default, so
/// no explicit asset injection is needed — only the planner instructions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannerSpawnSpec {
    pub task_id: String,
    pub cwd: PathBuf,
    /// Planner instructions appended via `--append-system-prompt`: clarify the
    /// task with the user and produce the DAG via `fleet task update-plan`.
    pub system_prompt: String,
    pub model: String,
}

/// A review session: isolated (safe-mode + bypassPermissions +
/// no-session-persistence), no interaction. Its `system_prompt` carries the
/// transplanted assets (engineering principles + relevant memory) plus the
/// acceptance criteria and the worker's diff/summary to judge against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewSpawnSpec {
    pub task_id: String,
    pub p_item_id: String,
    pub cwd: PathBuf,
    pub system_prompt: String,
    pub model: String,
}
