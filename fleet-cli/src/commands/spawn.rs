//! `fleet spawn` — start a detached session right now, in any workspace.
//!
//! The immediate sibling of `fleet schedule` (fires once, later) and `fleet
//! loop` (fires repeatedly). Until this existed, "go do that work over in the
//! other repo, now" had no verb at all: the only spawning commands were the
//! future-tense ones, so an agent reached for `fleet schedule create --in 60s`
//! and then waited out the 60s floor for a session it wanted immediately.
//!
//! Like the "New Session" button it stamps `NEW_SESSION_ENTRYPOINT`, so the
//! session it produces is Fleet-owned: it shows on the task page and can be
//! interrupted (`fleet interrupt`) and steered (`fleet send`).

use crate::commands::session::{
    inherit_context_maybe_scanning, resolve_session_id, resolve_workspace_flag,
};

/// Spawn a session and print what was started — session id, workspace, model
/// and harness. The id is the whole point of printing: it is what `fleet send`
/// / `fleet interrupt` / `fleet agent` take, so the caller never has to go
/// hunting through transcript mtimes to find out what it just launched.
#[allow(clippy::too_many_arguments)]
pub(crate) fn cmd_spawn(
    prompt: &str,
    workspace: Option<&str>,
    title: Option<&str>,
    model: Option<&str>,
    effort: Option<&str>,
    session: Option<&str>,
) {
    let prompt = prompt.trim();
    if prompt.is_empty() {
        eprintln!("Error: --prompt is required.");
        std::process::exit(2);
    }
    // Same inheritance as `fleet schedule create`: model / effort / harness come
    // from the creating session unless a flag overrides them. Only the workspace
    // now has an explicit override too.
    let sid = resolve_session_id(session);
    let ctx = inherit_context_maybe_scanning(sid.as_deref(), session.is_some());
    let workspace_path = match resolve_workspace_flag(workspace, &ctx) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    };
    let route = match claw_fleet_core::agent_source::route_launch(&ctx, model, effort) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::exit(2);
        }
    };

    let resp = match claw_fleet_core::agent_source::spawn_session(
        &route.agent_source,
        &claw_fleet_core::agent_source::SpawnSpec {
            workspace_path: workspace_path.clone(),
            prompt: prompt.to_string(),
            model: route.model.clone(),
            effort: route.effort.clone(),
            permission_mode: None,
            session_id: None,
            entrypoint: claw_fleet_core::session_launch::NEW_SESSION_ENTRYPOINT.to_string(),
            images: Vec::new(),
        },
    ) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("Error: spawn failed: {e}");
            std::process::exit(1);
        }
    };

    // A title given up front saves the new session from having to name itself,
    // and more importantly labels it in the task list from the first second —
    // before it has produced any output to name it by.
    if let (Some(t), Some(new_sid)) = (title.map(str::trim).filter(|t| !t.is_empty()), resp
        .session_id
        .as_deref())
    {
        if let Err(e) =
            claw_fleet_core::session_title::set_title(new_sid, &workspace_path, Some(t.to_string()))
        {
            eprintln!("warning: could not set title: {e}");
        }
    }

    let ws_name = claw_fleet_core::wiki::workspace_name_of(&workspace_path);
    match resp.session_id.as_deref() {
        Some(new_sid) => println!(
            "ok: session {new_sid} started in {ws_name} ({workspace_path}), model={}{}, pid {}.\n\
             操控用 `fleet send {new_sid} <话>` / `fleet interrupt {new_sid}`。",
            route.model.as_deref().unwrap_or("<CLI 默认>"),
            route.switch_note(),
            resp.pid,
        ),
        // Codex mints its own rollout id, so there is nothing to print yet; the
        // scanner picks the session up once the rollout file appears.
        None => println!(
            "ok: session started in {ws_name} ({workspace_path}), model={}{}, pid {}.",
            route.model.as_deref().unwrap_or("<CLI 默认>"),
            route.switch_note(),
            resp.pid,
        ),
    }
}
