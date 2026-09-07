use super::*;

// ── LLM provider commands ──────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn list_llm_providers(state: tauri::State<AppState>) -> Vec<llm_provider::LlmProviderInfo> {
    state.cached_llm_providers.lock().unwrap().clone()
}

#[tauri::command(async)]
pub(crate) fn get_llm_config(state: tauri::State<'_, AppState>) -> llm_provider::LlmConfig {
    state.backend.get_llm_config()
}

#[tauri::command(async)]
pub(crate) fn set_llm_config(state: tauri::State<'_, AppState>, config: llm_provider::LlmConfig) -> Result<(), String> {
    // Update both AppState (for background threads) and LocalBackend.
    *state.llm_config.lock().unwrap() = config.clone();
    state.backend.set_llm_config(config)
}

#[tauri::command(async)]
pub(crate) fn list_fleet_llm_usage_daily(
    from_ms: u64,
    to_ms: u64,
    state: tauri::State<'_, AppState>,
) -> Vec<llm_usage::FleetLlmUsageDailyBucket> {
    state
        .backend
        .list_fleet_llm_usage_daily(from_ms, to_ms)
}

#[tauri::command(async)]
pub(crate) fn get_usage_history(
    from_ms: i64,
    to_ms: i64,
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::account::UsageHistoryPoint> {
    state.backend.usage_history(from_ms, to_ms)
}

#[tauri::command(async)]
pub(crate) fn get_codex_usage_history(
    from_ms: i64,
    to_ms: i64,
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::codex_usage_history::CodexUsageHistoryPoint> {
    state
        .backend
        .codex_usage_history(from_ms, to_ms)
}

