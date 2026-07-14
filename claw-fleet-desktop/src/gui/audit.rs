use super::*;
use crate::audit;
use serde::Serialize;

// ── Security audit ──────────────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn get_audit_events(state: tauri::State<AppState>) -> audit::AuditSummary {
    state.backend.read().unwrap().get_audit_events()
}

#[tauri::command]
pub(crate) fn get_audit_rules(state: tauri::State<AppState>) -> Vec<audit::AuditRuleInfo> {
    state.backend.read().unwrap().get_audit_rules()
}

#[tauri::command]
pub(crate) fn set_audit_rule_enabled(state: tauri::State<AppState>, id: String, enabled: bool) -> Result<(), String> {
    state.backend.read().unwrap().set_audit_rule_enabled(&id, enabled)
}

#[tauri::command]
pub(crate) fn save_custom_audit_rule(state: tauri::State<AppState>, rule: audit::AuditRuleInfo) -> Result<(), String> {
    state.backend.read().unwrap().save_custom_audit_rule(rule)
}

#[tauri::command]
pub(crate) fn delete_custom_audit_rule(state: tauri::State<AppState>, id: String) -> Result<(), String> {
    state.backend.read().unwrap().delete_custom_audit_rule(&id)
}

#[tauri::command]
pub(crate) fn suggest_audit_rules(state: tauri::State<AppState>, concern: String, lang: String) -> Result<Vec<audit::SuggestedRule>, String> {
    state.backend.read().unwrap().suggest_audit_rules(&concern, &lang)
}

#[tauri::command]
pub(crate) fn get_daily_report(
    date: String,
    state: tauri::State<AppState>,
) -> Result<Option<daily_report::DailyReport>, String> {
    state.backend.read().unwrap().get_daily_report(&date)
}

#[tauri::command]
pub(crate) fn list_daily_report_stats(
    from: String,
    to: String,
    state: tauri::State<AppState>,
) -> Vec<daily_report::DailyReportStats> {
    state.backend.read().unwrap().list_daily_report_stats(&from, &to)
}

#[tauri::command]
pub(crate) async fn generate_daily_report(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<daily_report::DailyReport, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.read().unwrap().generate_daily_report(&date)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) async fn generate_daily_report_ai_summary(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.read().unwrap().generate_daily_report_ai_summary(&date)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) async fn generate_daily_report_lessons(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::daily_report::Lesson>, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.read().unwrap().generate_daily_report_lessons(&date)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) async fn append_lesson_to_claude_md(
    lesson: crate::daily_report::Lesson,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.read().unwrap().append_lesson_to_claude_md(&lesson)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) fn check_pattern_update() -> String {
    pattern_update::check_update_now()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatternInfo {
    version: u32,
    path: String,
}

#[tauri::command]
pub(crate) fn get_pattern_info() -> PatternInfo {
    let (version, path) = pattern_update::get_patterns_info();
    PatternInfo { version, path }
}

#[tauri::command]
pub(crate) fn start_watching_session(
    jsonl_path: String,
    state: tauri::State<AppState>,
) -> Result<u64, String> {
    state.backend.read().unwrap().start_watch(jsonl_path)
}

#[tauri::command]
pub(crate) fn stop_watching_session(state: tauri::State<AppState>) {
    state.backend.read().unwrap().stop_watch();
}

