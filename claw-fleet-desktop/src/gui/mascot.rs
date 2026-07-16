use super::*;

// ── Mascot quip generation ──────────────────────────────────────────────────

#[tauri::command]
pub(crate) async fn generate_mascot_quips(
    state: tauri::State<'_, AppState>,
    busy_titles: Vec<String>,
    done_titles: Vec<String>,
    locale: String,
) -> Result<claude_analyze::MascotQuips, String> {
    let cfg = state.llm_config.lock().unwrap().clone();
    Ok(tokio::task::spawn_blocking(move || {
        claude_analyze::generate_mascot_quips_routed(
            &cfg, &busy_titles, &done_titles, &locale,
        )
    })
    .await
    .unwrap_or_default())
}
