use super::blocking::{run_blocking, run_blocking_result};
use super::*;

// ── Tauri commands ───────────────────────────────────────────────────────────
//
// The transcript / usage readers below all run through `run_blocking` rather
// than sitting on an async-runtime worker: they read files, take core's global
// caches' locks, or both, and there are only `num_cpus` workers to go round.
// See `gui::blocking` for the cold-start freeze that taught us this.

#[tauri::command]
pub(crate) async fn list_sessions(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<SessionInfo>, String> {
    let backend = state.backend.clone();
    run_blocking(move || backend.list_sessions()).await
}

#[tauri::command]
pub(crate) async fn today_usage(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::today_usage::TodayUsage, String> {
    let backend = state.backend.clone();
    run_blocking(move || backend.today_usage()).await
}

#[tauri::command]
pub(crate) async fn today_usage_breakdown(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::today_usage::TodayUsageBreakdown, String> {
    let backend = state.backend.clone();
    run_blocking(move || backend.today_usage_breakdown()).await
}

#[tauri::command]
pub(crate) async fn usage_range_breakdown(
    from_ms: i64,
    to_ms: i64,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::today_usage::UsageRangeBreakdown, String> {
    let backend = state.backend.clone();
    run_blocking(move || backend.usage_range_breakdown(from_ms, to_ms)).await
}

#[tauri::command]
pub(crate) async fn search_sessions(
    query: String,
    limit: Option<usize>,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<search_index::SearchHit>, String> {
    let limit = limit.unwrap_or(50);
    if query.trim().is_empty() {
        return Ok(vec![]);
    }
    let backend = state.backend.clone();
    run_blocking(move || backend.search_sessions(&query, limit)).await
}

#[tauri::command]
pub(crate) async fn get_messages(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Value>, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || backend.get_messages(&jsonl_path)).await
}

/// Read at most the last `tail` messages of a session. Used by SessionDetail
/// to avoid stalling the webview on large transcripts.
///
/// Runs on the blocking pool, not the main thread and not an async-runtime
/// worker: it reads whole transcript files (can be tens of MB) and is polled
/// every ~1.5s plus fired on every decision-card mount. Off the main thread so
/// it can't stall paints / other IPC (a source of decision-panel submit jank);
/// off the async workers so it can't be *starved by* them — this is the call
/// that sat behind 23 other blocked bodies during the 2026-09-10 cold start and
/// left task detail on 「加载中…」 for ~47s.
#[tauri::command]
pub(crate) async fn get_messages_tail(
    jsonl_path: String,
    tail: usize,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<Value>, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || get_messages_tail_inner(jsonl_path, tail, &backend)).await
}

fn get_messages_tail_inner(
    jsonl_path: String,
    tail: usize,
    backend: &std::sync::Arc<local_backend::LocalBackend>,
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
    let probe = crate::cmd_probe::CmdProbe::start("get_messages_tail", &jsonl_path);
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

/// One step of a live follow: everything appended since byte `offset`, plus the
/// cursor to pass next time. `offset: None` returns no messages, only where the
/// transcript currently ends — how a follower gets its first cursor.
///
/// This is what a detail pane polls instead of re-requesting a whole window.
/// The window path re-read a 4513-record transcript every 1.5s and took 1–3s
/// doing it (see `liveTailWindow.ts`); a cursor makes the steady-state read
/// proportional to what the agent just wrote.
///
/// On the blocking pool for the same reason as `get_messages_tail` — it
/// touches the file.
#[tauri::command]
pub(crate) async fn get_messages_since(
    jsonl_path: String,
    offset: Option<u64>,
    state: tauri::State<'_, AppState>,
) -> Result<TailDelta, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || {
        let probe = crate::cmd_probe::CmdProbe::start("get_messages_since", &jsonl_path);
        let out = backend.get_messages_since(&jsonl_path, offset);
        probe.done(|| match &out {
            Ok((msgs, off)) => format!("{} msgs, offset {off}", msgs.len()),
            Err(e) => format!("error: {e}"),
        });
        out.map(|(messages, offset)| TailDelta { messages, offset })
    })
    .await
}

/// Result of [`get_messages_since`]. A struct rather than a tuple so the
/// frontend reads `.messages` / `.offset` instead of `[0]` / `[1]`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TailDelta {
    messages: Vec<Value>,
    offset: u64,
}

/// Full, untrimmed tool output for one `tool_use_id`. `get_messages_tail`
/// truncates oversized tool output for transport; the frontend calls this when
/// the reader expands a card flagged `_fleetTruncated`. On the blocking pool
/// for the same reason as `get_messages_tail` — it reads the whole transcript.
#[tauri::command]
pub(crate) async fn get_tool_result_full(
    jsonl_path: String,
    tool_use_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Value, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || backend.get_tool_result_full(&jsonl_path, &tool_use_id)).await
}

#[tauri::command]
pub(crate) async fn get_skill_history(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::skill_history::SkillInvocation>, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || backend.get_skill_history(&jsonl_path)).await
}

#[tauri::command]
pub(crate) async fn get_workflow_trees(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::workflow::WorkflowTree>, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || backend.get_workflow_trees(&jsonl_path)).await
}

#[tauri::command]
pub(crate) async fn get_task_token_breakdown(
    jsonl_path: String,
    project_root: Option<String>,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::token_analysis::TaskTokenBreakdown, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || {
        backend.get_task_token_breakdown(&jsonl_path, project_root.as_deref())
    })
    .await
}

#[tauri::command]
pub(crate) async fn get_codex_token_breakdown(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::codex_source::CodexTokenBreakdown, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || backend.get_codex_token_breakdown(&jsonl_path)).await
}

/// `uri` is a `dsh://<session-id>`, not a path — dsh sessions have no file.
#[tauri::command]
pub(crate) async fn get_dsh_token_breakdown(
    uri: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::dsh_source::DshTokenBreakdown, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || backend.get_dsh_token_breakdown(&uri)).await
}

/// Real spend for a `dsh://` session. On the blocking pool because it may go to
/// the provider over the network.
#[tauri::command]
pub(crate) async fn get_dsh_session_cost(
    uri: String,
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::dsh_cost::DshSessionCost, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || backend.get_dsh_session_cost(&uri)).await
}

/// dsh's model catalogue for the launcher's model / effort menus. On the
/// blocking pool because the first call may have to start `dsh web` and then
/// wait on it — the single longest thing a command here can do.
#[tauri::command]
pub(crate) async fn dsh_models(
    state: tauri::State<'_, AppState>,
) -> Result<claw_fleet_core::dsh_source::DshModelCatalog, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || backend.dsh_models()).await
}

/// Fleet's own model catalog (`models.toml`) for the launcher's menus.
///
/// Cheap and synchronous — the catalog is parsed once per process and the
/// availability probe reads the sources config. Unlike `dsh_models` there is no
/// server to start.
#[tauri::command]
pub(crate) fn model_catalog(
    state: tauri::State<'_, AppState>,
) -> Vec<claw_fleet_core::model_catalog::PickerHarness> {
    state.backend.model_catalog()
}

#[tauri::command]
pub(crate) async fn get_session_todos(
    jsonl_path: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::session_todos::TodoItem>, String> {
    let backend = state.backend.clone();
    run_blocking_result(move || {
        let messages = backend.get_messages(&jsonl_path)?;
        Ok(claw_fleet_core::session_todos::extract_latest_todos(&messages))
    })
    .await
}


/// Images a Codex session generated, for the session detail's thumbnail strip.
///
/// A Codex session's Fleet id *is* its Codex thread id, which is also the output
/// directory's name — so no correlation table is needed, and a Claude session
/// simply comes back empty. Bytes reach the webview through the
/// `fleet-genimage://` protocol registered in `gui/mod.rs`.
#[tauri::command]
pub(crate) async fn list_session_images(
    session_id: String,
    state: tauri::State<'_, AppState>,
) -> Result<Vec<claw_fleet_core::codex_image::GeneratedImage>, String> {
    let backend = state.backend.clone();
    run_blocking(move || backend.list_session_images(&session_id)).await
}
