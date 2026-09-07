use super::*;
use crate::audit;
use serde::Serialize;

// ── Security audit ──────────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn get_audit_events(state: tauri::State<'_, AppState>) -> audit::AuditSummary {
    state.backend.get_audit_events()
}

#[tauri::command(async)]
pub(crate) fn get_audit_rules(state: tauri::State<'_, AppState>) -> Vec<audit::AuditRuleInfo> {
    state.backend.get_audit_rules()
}

#[tauri::command(async)]
pub(crate) fn set_audit_rule_enabled(state: tauri::State<'_, AppState>, id: String, enabled: bool) -> Result<(), String> {
    state.backend.set_audit_rule_enabled(&id, enabled)
}

#[tauri::command(async)]
pub(crate) fn save_custom_audit_rule(state: tauri::State<'_, AppState>, rule: audit::AuditRuleInfo) -> Result<(), String> {
    state.backend.save_custom_audit_rule(rule)
}

#[tauri::command(async)]
pub(crate) fn delete_custom_audit_rule(state: tauri::State<'_, AppState>, id: String) -> Result<(), String> {
    state.backend.delete_custom_audit_rule(&id)
}

#[tauri::command(async)]
pub(crate) fn suggest_audit_rules(state: tauri::State<'_, AppState>, concern: String, lang: String) -> Result<Vec<audit::SuggestedRule>, String> {
    state.backend.suggest_audit_rules(&concern, &lang)
}

#[tauri::command(async)]
pub(crate) fn get_daily_report(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<Option<daily_report::DailyReport>, String> {
    state.backend.get_daily_report(&date)
}

#[tauri::command(async)]
pub(crate) fn list_daily_report_stats(
    from: String,
    to: String,
    state: tauri::State<'_, AppState>,
) -> Vec<daily_report::DailyReportStats> {
    state.backend.list_daily_report_stats(&from, &to)
}

#[tauri::command]
pub(crate) async fn generate_daily_report(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<daily_report::DailyReport, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.generate_daily_report(&date)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) async fn generate_daily_report_ai_summary(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<String, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.generate_daily_report_ai_summary(&date)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) async fn generate_daily_report_lessons(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::daily_report::Lesson>, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.generate_daily_report_lessons(&date)
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) async fn append_lesson_to_claude_md(
    lesson: crate::daily_report::Lesson,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let backend = state.backend.clone();
    let title = state.user_title.lock().unwrap().clone();
    let locale = state.locale.lock().unwrap().clone();
    tokio::task::spawn_blocking(move || {
        backend.append_lesson_to_claude_md(&lesson)?;
        // Mirror onto codex AGENTS.md too (no-op when codex isn't in use).
        let _ = backend.reconcile_codex_guidance(&title, &locale);
        Ok(())
    }).await.map_err(|e| format!("join: {e}"))?
}

/// The day's per-task retrospectives, for the report's task-review card.
/// `spawn_blocking` like its neighbours: this is a SQLite read, and blocking the
/// async runtime on disk I/O is what freezes the whole webview.
#[tauri::command]
pub(crate) async fn list_task_reviews(
    date: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::task_review::TaskReview>, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.list_task_reviews(&date)
    }).await.map_err(|e| format!("join: {e}"))
}

#[tauri::command]
pub(crate) async fn list_managed_lessons(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::lessons_store::ManagedLesson>, String> {
    let backend = state.backend.clone();
    tokio::task::spawn_blocking(move || {
        backend.list_managed_lessons()
    }).await.map_err(|e| format!("join: {e}"))?
}

#[tauri::command]
pub(crate) async fn remove_managed_lesson(
    id: String,
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let backend = state.backend.clone();
    let title = state.user_title.lock().unwrap().clone();
    let locale = state.locale.lock().unwrap().clone();
    tokio::task::spawn_blocking(move || {
        backend.remove_managed_lesson(&id)?;
        // Re-sync codex AGENTS.md so the removed lesson drops there too.
        let _ = backend.reconcile_codex_guidance(&title, &locale);
        Ok(())
    }).await.map_err(|e| format!("join: {e}"))?
}

// Threadpool, not the main thread: `check_update_now` does a blocking HTTP
// fetch of the remote pattern manifest. As a plain `#[tauri::command] fn` it
// ran inline on the event loop (see `cmd_probe` / tauri-macros
// `ExecutionContext::Blocking`), so a slow or dead network froze the whole UI
// for the length of the request.
#[tauri::command(async)]
pub(crate) fn check_pattern_update() -> String {
    pattern_update::check_update_now()
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PatternInfo {
    version: u32,
    path: String,
}

#[tauri::command(async)]
pub(crate) fn get_pattern_info() -> PatternInfo {
    let (version, path) = pattern_update::get_patterns_info();
    PatternInfo { version, path }
}

#[tauri::command(async)]
pub(crate) fn start_watching_session(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<u64, String> {
    // Watched, not just timed: this takes the backend **write** lock while the
    // detail pane's read-lock pollers are running, and a call that never returns
    // would never reach a completion log. It fires once per session open, so a
    // watchdog thread here is cheap. See `cmd_probe`.
    let probe = crate::cmd_probe::CmdProbe::start_watched("start_watching_session", &jsonl_path);
    let backend = &state.backend;
    let out = backend.start_watch(jsonl_path);
    probe.done(|| match &out {
        Ok(size) => format!("watching from byte {size}"),
        Err(e) => format!("error: {e}"),
    });
    out
}

#[tauri::command(async)]
pub(crate) fn stop_watching_session(state: tauri::State<'_, AppState>) {
    // Same write lock, and it runs *before* the fetch on every `open()` — a hang
    // here delays the pane before it ever says 「加载中…」.
    let probe = crate::cmd_probe::CmdProbe::start_watched("stop_watching_session", "");
    let backend = &state.backend;
    backend.stop_watch();
    probe.done(|| "stopped".into());
}

