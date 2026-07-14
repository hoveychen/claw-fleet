use super::*;
use crate::memory;

// ── Memory commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn list_memories(state: tauri::State<AppState>) -> Vec<memory::WorkspaceMemory> {
    state.backend.read().unwrap().list_memories()
}

/// `(async)` → threadpool: polled every 700ms while a session streams. Even
/// after the stale-skip fix it still does a `readdir` + `stat` per sidecar, so
/// keep it off the main thread. The body stays synchronous.
#[tauri::command(async)]
pub(crate) fn read_live_thinking(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Option<claw_fleet_core::live_thinking::LiveThinking> {
    state.backend.read().unwrap().read_live_thinking(&session_id)
}

#[tauri::command]
pub(crate) fn get_memory_content(path: String, state: tauri::State<AppState>) -> Result<String, String> {
    state.backend.read().unwrap().get_memory_content(&path)
}

#[tauri::command]
pub(crate) fn get_memory_history(path: String, state: tauri::State<AppState>) -> Vec<memory::MemoryHistoryEntry> {
    state.backend.read().unwrap().get_memory_history(&path)
}

#[tauri::command]
pub(crate) fn get_task_plans(
    workspace_path: String,
    session_id: Option<String>,
    state: tauri::State<AppState>,
) -> Vec<claw_fleet_core::prd_tasks::TaskPlanDetail> {
    state
        .backend
        .read()
        .unwrap()
        .get_task_plans(&workspace_path, session_id.as_deref())
}

#[tauri::command]
pub(crate) fn get_claude_md_content(workspace_path: String) -> Result<String, String> {
    memory::read_claude_md(&workspace_path)
}

#[tauri::command]
pub(crate) fn promote_memory(memory_path: String, target: String, workspace_path: String) -> Result<(), String> {
    memory::promote_memory(&memory_path, &target, &workspace_path)
}

