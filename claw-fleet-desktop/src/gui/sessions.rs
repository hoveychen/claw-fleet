use super::*;

// ── Tauri commands ───────────────────────────────────────────────────────────

#[tauri::command(async)]
pub(crate) fn list_sessions(state: tauri::State<'_, AppState>) -> Vec<SessionInfo> {
    state.backend.read().unwrap().list_sessions()
}

#[tauri::command(async)]
pub(crate) fn today_usage(state: tauri::State<'_, AppState>) -> claw_fleet_core::today_usage::TodayUsage {
    state.backend.read().unwrap().today_usage()
}

#[tauri::command(async)]
pub(crate) fn today_usage_breakdown(
    state: tauri::State<'_, AppState>,
) -> claw_fleet_core::today_usage::TodayUsageBreakdown {
    state.backend.read().unwrap().today_usage_breakdown()
}

#[tauri::command(async)]
pub(crate) fn usage_range_breakdown(
    from_ms: i64,
    to_ms: i64,
    state: tauri::State<'_, AppState>,
) -> claw_fleet_core::today_usage::UsageRangeBreakdown {
    state
        .backend
        .read()
        .unwrap()
        .usage_range_breakdown(from_ms, to_ms)
}

#[tauri::command(async)]
pub(crate) fn search_sessions(
    query: String,
    limit: Option<usize>,
    state: tauri::State<'_, AppState>,
) -> Vec<search_index::SearchHit> {
    let limit = limit.unwrap_or(50);
    if query.trim().is_empty() {
        return vec![];
    }
    state.backend.read().unwrap().search_sessions(&query, limit)
}

#[tauri::command(async)]
pub(crate) fn get_messages(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Value>, String> {
    state.backend.read().unwrap().get_messages(&jsonl_path)
}

/// Read at most the last `tail` messages of a session. Used by SessionDetail
/// to avoid stalling the webview on large transcripts.
///
/// `(async)` runs this on Tauri's threadpool instead of the main thread: it
/// reads whole transcript files (can be tens of MB) and is polled every ~1.5s
/// plus fired on every decision-card mount, so keeping it off the main thread
/// prevents it from stalling paints / other IPC (a source of decision-panel
/// submit jank). The body stays synchronous.
#[tauri::command(async)]
pub(crate) fn get_messages_tail(
    jsonl_path: String,
    tail: usize,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Value>, String> {
    // Probed: this is the call behind 「对话」Tab, and when it goes slow the log
    // has to say whether the time went to the backend or to queueing for
    // `AppState::backend`. See `cmd_probe`.
    //
    // dsh:// additionally logs unconditionally at entry AND completion. The
    // slow-only probe is blind to the two states the eternal 「加载中…」 hunt
    // must distinguish: a call that never arrived and a call that finished
    // under the 1s threshold both leave zero lines. These entry/exit lines are
    // the missing half of the split — the frontend logs the same request id on
    // its side (SessionDetail's standalone fetch), so the log shows exactly
    // which hop of webview → IPC → command → response went dark.
    let is_dsh = jsonl_path.starts_with("dsh://");
    if is_dsh {
        claw_fleet_core::log_debug(&format!(
            "get_messages_tail[dsh] enter tail={tail} [{jsonl_path}]"
        ));
    }
    let mut probe = crate::cmd_probe::CmdProbe::start("get_messages_tail", &jsonl_path);
    let backend = state.backend.read().unwrap();
    probe.locked();
    let out = backend.get_messages_tail(&jsonl_path, tail);
    probe.done(|| match &out {
        Ok(msgs) => format!("{} msgs", msgs.len()),
        Err(e) => format!("error: {e}"),
    });
    if is_dsh {
        claw_fleet_core::log_debug(&format!(
            "get_messages_tail[dsh] done {} [{jsonl_path}]",
            match &out {
                Ok(msgs) => format!("{} msgs", msgs.len()),
                Err(e) => format!("error: {e}"),
            }
        ));
    }
    out
}

/// Full, untrimmed tool output for one `tool_use_id`. `get_messages_tail`
/// truncates oversized tool output for transport; the frontend calls this when
/// the reader expands a card flagged `_fleetTruncated`. `(async)` for the same
/// reason as `get_messages_tail` — it reads the whole transcript.
#[tauri::command(async)]
pub(crate) fn get_tool_result_full(
    jsonl_path: String,
    tool_use_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Value, String> {
    state
        .backend
        .read()
        .unwrap()
        .get_tool_result_full(&jsonl_path, &tool_use_id)
}

#[tauri::command(async)]
pub(crate) fn get_skill_history(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::skill_history::SkillInvocation>, String> {
    state.backend.read().unwrap().get_skill_history(&jsonl_path)
}

#[tauri::command(async)]
pub(crate) fn get_workflow_trees(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::workflow::WorkflowTree>, String> {
    state
        .backend
        .read()
        .unwrap()
        .get_workflow_trees(&jsonl_path)
}

#[tauri::command(async)]
pub(crate) fn get_task_token_breakdown(
    jsonl_path: String,
    project_root: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::token_analysis::TaskTokenBreakdown, String> {
    state
        .backend
        .read()
        .unwrap()
        .get_task_token_breakdown(&jsonl_path, project_root.as_deref())
}

#[tauri::command(async)]
pub(crate) fn get_codex_token_breakdown(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::codex_source::CodexTokenBreakdown, String> {
    state
        .backend
        .read()
        .unwrap()
        .get_codex_token_breakdown(&jsonl_path)
}

/// `uri` is a `dsh://<session-id>`, not a path — dsh sessions have no file.
#[tauri::command(async)]
pub(crate) fn get_dsh_token_breakdown(
    uri: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::dsh_source::DshTokenBreakdown, String> {
    state
        .backend
        .read()
        .unwrap()
        .get_dsh_token_breakdown(&uri)
}

/// Real spend for a `dsh://` session. `async` because it may go to the provider
/// over the network — a sync command would block the main thread.
#[tauri::command(async)]
pub(crate) fn get_dsh_session_cost(
    uri: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::dsh_cost::DshSessionCost, String> {
    state.backend.read().unwrap().get_dsh_session_cost(&uri)
}

/// dsh's model catalogue for the launcher's model / effort menus. `async`
/// because the first call may have to start `dsh web` and then wait on it.
#[tauri::command(async)]
pub(crate) fn dsh_models(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::dsh_source::DshModelCatalog, String> {
    state.backend.read().unwrap().dsh_models()
}

#[tauri::command(async)]
pub(crate) fn get_session_todos(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::session_todos::TodoItem>, String> {
    let messages = state.backend.read().unwrap().get_messages(&jsonl_path)?;
    Ok(claw_fleet_core::session_todos::extract_latest_todos(&messages))
}

