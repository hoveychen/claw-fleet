use super::*;

// ── Decision panel timeouts ──────────────────────────────────────────────────

#[tauri::command]
pub(crate) fn get_decision_panel_config() -> claw_fleet_core::decision_panel_config::DecisionPanelConfig {
    claw_fleet_core::decision_panel_config::load()
}

#[tauri::command]
pub(crate) fn set_decision_panel_config(
    cfg: claw_fleet_core::decision_panel_config::DecisionPanelConfig,
) -> Result<claw_fleet_core::decision_panel_config::DecisionPanelConfig, String> {
    let mut cfg = cfg;
    claw_fleet_core::decision_panel_config::save(&mut cfg).map_err(|e| e.to_string())?;
    Ok(cfg)
}

/// Live-fetch the body of a review doc attached to a `fleet__ask` card, so the
/// card's side panel can render it as a tab. Goes through the backend, so it
/// resolves wiki slugs / local files on whichever host serves the session.
#[tauri::command(async)]
pub(crate) fn read_review_doc(
    doc: claw_fleet_core::mcp_ipc::ReviewDoc,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::mcp_ipc::ReviewDocContent, String> {
    state.backend.read().unwrap().read_review_doc(&doc)
}

