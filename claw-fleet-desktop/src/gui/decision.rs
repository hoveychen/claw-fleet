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

