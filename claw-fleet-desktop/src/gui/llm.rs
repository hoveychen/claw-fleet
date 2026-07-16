use super::*;

// ── LLM provider commands ──────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn list_llm_providers(state: tauri::State<AppState>) -> Vec<llm_provider::LlmProviderInfo> {
    state.cached_llm_providers.lock().unwrap().clone()
}

#[tauri::command]
pub(crate) fn get_llm_config(state: tauri::State<AppState>) -> llm_provider::LlmConfig {
    state.backend.read().unwrap().get_llm_config()
}

#[tauri::command]
pub(crate) fn set_llm_config(state: tauri::State<AppState>, config: llm_provider::LlmConfig) -> Result<(), String> {
    // Update both AppState (for background threads) and Backend.
    *state.llm_config.lock().unwrap() = config.clone();
    state.backend.read().unwrap().set_llm_config(config)
}

#[tauri::command]
pub(crate) fn list_fleet_llm_usage_daily(
    from_ms: u64,
    to_ms: u64,
    state: tauri::State<AppState>,
) -> Vec<llm_usage::FleetLlmUsageDailyBucket> {
    state
        .backend
        .read()
        .unwrap()
        .list_fleet_llm_usage_daily(from_ms, to_ms)
}

#[tauri::command]
pub(crate) fn get_usage_history(
    from_ms: i64,
    to_ms: i64,
    state: tauri::State<AppState>,
) -> Vec<claw_fleet_core::account::UsageHistoryPoint> {
    state.backend.read().unwrap().usage_history(from_ms, to_ms)
}

#[tauri::command]
pub(crate) fn get_codex_usage_history(
    from_ms: i64,
    to_ms: i64,
    state: tauri::State<AppState>,
) -> Vec<claw_fleet_core::codex_usage_history::CodexUsageHistoryPoint> {
    state
        .backend
        .read()
        .unwrap()
        .codex_usage_history(from_ms, to_ms)
}

