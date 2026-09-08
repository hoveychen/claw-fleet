use super::*;
use crate::memory;

// ── Memory commands ──────────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn list_memories(state: tauri::State<'_, AppState>) -> Vec<memory::WorkspaceMemory> {
    state.backend.list_memories()
}

/// `(async)` → threadpool: polled every 700ms while a session streams. Even
/// after the stale-skip fix it still does a `readdir` + `stat` per sidecar, so
/// keep it off the main thread. The body stays synchronous.
#[tauri::command(async)]
pub(crate) fn read_live_thinking(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Option<claw_fleet_core::live_thinking::LiveThinking> {
    state.backend.read_live_thinking(&session_id)
}

#[tauri::command(async)]
pub(crate) fn get_memory_content(path: String, state: tauri::State<'_, AppState>) -> Result<String, String> {
    state.backend.get_memory_content(&path)
}

#[tauri::command(async)]
pub(crate) fn get_memory_history(path: String, state: tauri::State<'_, AppState>) -> Vec<memory::MemoryHistoryEntry> {
    state.backend.get_memory_history(&path)
}

#[tauri::command(async)]
pub(crate) fn get_task_plans(
    workspace_path: String,
    session_id: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::prd_tasks::TaskPlanDetail> {
    state
        .backend
        .get_task_plans(&workspace_path, session_id.as_deref())
}

/// The workspace's whole execution chain — plan forest with handoff chains
/// folded onto their plan nodes. Backs the top-level 计划树 view.
#[tauri::command(async)]
pub(crate) fn get_plan_forest(
    workspace_path: String,
    state: tauri::State<'_, AppState>,
) -> claw_fleet_core::plan_forest::PlanForest {
    state.backend.get_plan_forest(&workspace_path)
}

/// `(async)`: reading a workspace's CLAUDE.md is disk work of unbounded size
/// (these files run to tens of KB and the read happens on every Memory-view
/// open), and a read has nothing the event loop needs to serialize.
#[tauri::command(async)]
pub(crate) fn get_claude_md_content(workspace_path: String) -> Result<String, String> {
    memory::read_claude_md(&workspace_path)
}

/// Deliberately still a plain sync command, i.e. on the main thread.
///
/// This appends to the user's CLAUDE.md — a read-modify-write on a file the
/// user also edits by hand. The event loop is that write's only serialization
/// today, and a single append is milliseconds, so the trade the rest of this
/// sweep makes (leave the loop, accept concurrency) is the wrong one here.
#[tauri::command]
pub(crate) fn promote_memory(memory_path: String, target: String, workspace_path: String) -> Result<(), String> {
    memory::promote_memory(&memory_path, &target, &workspace_path)
}

