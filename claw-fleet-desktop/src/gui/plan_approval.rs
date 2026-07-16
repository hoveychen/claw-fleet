use super::*;

// ── Plan approval (ExitPlanMode interception) ───────────────────────────────

#[tauri::command(async)]
pub(crate) fn apply_plan_approval_hook(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().apply_plan_approval_hook()
}

#[tauri::command(async)]
pub(crate) fn remove_plan_approval_hook(state: tauri::State<'_, AppState>) -> Result<(), String> {
    state.backend.write().unwrap().remove_plan_approval_hook()
}

#[tauri::command(async)]
pub(crate) fn list_pending_plan_approvals(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::plan_approval::PlanApprovalRequest> {
    state.backend.read().unwrap().list_pending_plan_approvals()
}

#[tauri::command(async)]
pub(crate) fn respond_to_plan_approval(
    state: tauri::State<'_, AppState>,
    id: String,
    decision: String,
    edited_plan: Option<String>,
    feedback: Option<String>,
) -> Result<(), String> {
    state
        .backend
        .write()
        .unwrap()
        .respond_to_plan_approval(&id, &decision, edited_plan, feedback)
}

/// `(async)` → threadpool: `sync_user_prompts_from_jsonl` reads the entire
/// transcript with `fs::read_to_string`, and this fires on every decision-card
/// mount (the history strip). Off the main thread it can't stall the just-
/// rendered card. The body stays synchronous.
#[tauri::command(async)]
pub(crate) fn list_session_decisions(
    state: tauri::State<'_, AppState>,
    session_id: String,
    jsonl_path: Option<String>,
) -> Vec<claw_fleet_core::decision_history::DecisionHistoryRecord> {
    state
        .backend
        .read()
        .unwrap()
        .list_session_decisions(&session_id, jsonl_path.as_deref())
}

/// Mount catch-up: return every decision-panel request currently awaiting a
/// response so the frontend can seed its store on launch. Covers the
/// cold-restart gap where the one-shot watcher emit was lost because no Tauri
/// listener was attached yet.
#[tauri::command(async)]
pub(crate) fn list_pending_decisions(
    state: tauri::State<'_, AppState>,
) -> claw_fleet_core::backend::PendingDecisions {
    state.backend.read().unwrap().list_pending_decisions()
}

#[tauri::command(async)]
pub(crate) fn get_mobile_relay_config(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::mobile_relay::MobileRelayConfig, String> {
    state.backend.read().unwrap().get_mobile_relay_config()
}

#[tauri::command(async)]
pub(crate) fn set_mobile_relay_config(
    state: tauri::State<'_, AppState>,
    cfg: claw_fleet_core::mobile_relay::MobileRelayConfig,
) -> Result<claw_fleet_core::mobile_relay::MobileRelayConfig, String> {
    state.backend.write().unwrap().set_mobile_relay_config(cfg)
}

#[tauri::command(async)]
pub(crate) fn rotate_mobile_relay_secret(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::mobile_relay::MobileRelayConfig, String> {
    state.backend.write().unwrap().rotate_mobile_relay_secret()
}

#[tauri::command(async)]
pub(crate) fn mobile_relay_status(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::mobile_relay::MobileRelayStatus, String> {
    state.backend.read().unwrap().mobile_relay_status()
}

#[tauri::command(async)]
pub(crate) fn mobile_relay_qr_svg(
    state: tauri::State<'_, AppState>,
    lang: Option<String>,
) -> Result<String, String> {
    state.backend.read().unwrap().mobile_relay_qr_svg(lang.as_deref())
}

/// Read the last non-tool-use assistant message from a session, for guard context.
#[tauri::command(async)]
pub(crate) fn get_guard_context(state: tauri::State<'_, AppState>, session_id: String) -> String {
    // Find the session by ID and read its messages.
    let backend = state.backend.read().unwrap();
    let sessions = backend.list_sessions();
    let session = sessions.iter().find(|s| s.id == session_id);
    let Some(session) = session else {
        return String::new();
    };
    let messages = match backend.get_messages(&session.jsonl_path) {
        Ok(msgs) => msgs,
        Err(_) => return String::new(),
    };

    // Walk backwards to find the last assistant message with text content
    // (not just tool_use blocks).
    for msg in messages.iter().rev() {
        if msg.get("type").and_then(|t| t.as_str()) != Some("assistant") {
            continue;
        }
        let Some(content) = msg
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(|c| c.as_array())
        else {
            continue;
        };

        // Collect text blocks, skip tool_use blocks.
        let text_parts: Vec<&str> = content
            .iter()
            .filter(|b| b.get("type").and_then(|t| t.as_str()) == Some("text"))
            .filter_map(|b| b.get("text").and_then(|t| t.as_str()))
            .collect();

        if !text_parts.is_empty() {
            let combined = text_parts.join("\n");
            // Truncate to ~2000 bytes for the LLM prompt. Use the char-safe
            // helper: a raw `&combined[..2000]` panics when byte 2000 lands
            // inside a multi-byte char (CJK), which crashed the whole app.
            return claw_fleet_core::guard::truncate_command(&combined, 2000);
        }
    }

    String::new()
}

