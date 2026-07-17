use super::*;
use crate::skills;

// ── Skills ────────────────────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn list_skills(state: tauri::State<'_, AppState>) -> Vec<skills::SkillItem> {
    state.backend.read().unwrap().list_skills()
}

#[tauri::command(async)]
pub(crate) fn skill_sync_inventory(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::skill_sync::SkillSyncEntry>, String> {
    state.backend.read().unwrap().skill_sync_inventory()
}

#[tauri::command(async)]
pub(crate) fn skill_sync_apply(
    state: tauri::State<'_, AppState>,
) -> Result<crate::skill_sync::SkillSyncReport, String> {
    state.backend.read().unwrap().skill_sync_apply()
}

#[tauri::command(async)]
pub(crate) fn skill_sync_adopt(
    path: String,
    state: tauri::State<'_, AppState>,
) -> Result<crate::skill_sync::SkillSyncReport, String> {
    state.backend.read().unwrap().skill_sync_adopt(&path)
}

#[tauri::command(async)]
pub(crate) fn skill_sync_unlink(
    slug: String,
    target: String,
    state: tauri::State<'_, AppState>,
) -> Result<crate::skill_sync::SkillSyncAction, String> {
    let target = match target.as_str() {
        "claude-code" => crate::skill_sync::SkillTarget::ClaudeCode,
        "codex" => crate::skill_sync::SkillTarget::Codex,
        _ => return Err(format!("unknown skill target: {target}")),
    };
    state.backend.read().unwrap().skill_sync_unlink(&slug, target)
}

#[tauri::command(async)]
pub(crate) fn get_skill_autosync(state: tauri::State<'_, AppState>) -> Result<bool, String> {
    state.backend.read().unwrap().get_skill_autosync()
}

#[tauri::command(async)]
pub(crate) fn set_skill_autosync(
    enabled: bool,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    state.backend.read().unwrap().set_skill_autosync(enabled)
}

#[tauri::command(async)]
pub(crate) fn get_skill_content(path: String, state: tauri::State<'_, AppState>) -> Result<String, String> {
    state.backend.read().unwrap().get_skill_content(&path)
}

#[tauri::command(async)]
pub(crate) fn list_skill_files(
    skill_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<skills::SkillFileEntry>, String> {
    state.backend.read().unwrap().list_skill_files(&skill_path)
}

#[tauri::command(async)]
pub(crate) fn delete_skill(skill_path: String, state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().delete_skill(&skill_path)
}
