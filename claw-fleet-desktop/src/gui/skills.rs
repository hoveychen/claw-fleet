use super::*;
use crate::skills;

// ── Skills ────────────────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn list_skills(state: tauri::State<AppState>) -> Vec<skills::SkillItem> {
    state.backend.read().unwrap().list_skills()
}

#[tauri::command]
pub(crate) fn get_skill_content(path: String, state: tauri::State<AppState>) -> Result<String, String> {
    state.backend.read().unwrap().get_skill_content(&path)
}

#[tauri::command]
pub(crate) fn list_skill_files(
    skill_path: String,
    state: tauri::State<AppState>,
) -> Result<Vec<skills::SkillFileEntry>, String> {
    state.backend.read().unwrap().list_skill_files(&skill_path)
}

#[tauri::command]
pub(crate) fn delete_skill(skill_path: String, state: tauri::State<AppState>) -> Result<(), String> {
    state.backend.read().unwrap().delete_skill(&skill_path)
}

