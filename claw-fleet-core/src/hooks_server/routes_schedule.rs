//! Route handlers for `fleet serve`'s future-task endpoints — agent loops
//! (`fleet loop`), one-shot schedules (`fleet schedule`), creating either as a
//! routine (`crate::routines`), and the project list routines are filed under. These read the
//! host's global `~/.fleet/loops` / `~/.fleet/schedules`, so unlike the
//! session-scoped handlers they don't touch `ctx.sources`.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_loops(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let body = serde_json::to_string(&crate::agent_loop::list()).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_schedules(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let body = serde_json::to_string(&crate::schedule::list()).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_loop_cancel(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let id = query.get("id").cloned().unwrap_or_default();
    let ok = crate::agent_loop::stop(&id);
    let body = serde_json::json!({ "ok": ok }).to_string();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_schedule_cancel(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let id = query.get("id").cloned().unwrap_or_default();
    let ok = crate::schedule::cancel(&id);
    let body = serde_json::json!({ "ok": ok }).to_string();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

/// Edit a pending schedule: apply the JSON `ScheduleUpdate` body, re-arm the
/// timer on *this* host (where the detached timer lives), and return the updated
/// record. Mirrors `Backend::update_schedule`'s local default.
pub(crate) fn route_schedule_update(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    match serde_json::from_slice::<crate::schedule::ScheduleUpdate>(&body_bytes) {
        Ok(update) => match crate::schedule::update(&update) {
            Ok(rec) => {
                let _ = crate::schedule::arm_timer(&rec);
                let body = serde_json::to_string(&rec).unwrap_or_default();
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_header(json_header));
            }
            Err(e) => {
                let body = format!(r#"{{"error":"{}"}}"#, e.replace('"', "\\\""));
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(500)
                        .with_header(json_header),
                );
            }
        },
        Err(e) => {
            let body = format!(r#"{{"error":"{}"}}"#, e.to_string().replace('"', "'"));
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(400)
                    .with_header(json_header),
            );
        }
    }
}

/// `POST /routines` — body is a [`crate::routines::RoutineSpec`]; answers the
/// [`crate::routines::CreatedRoutine`].
pub(crate) fn route_routine_create(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    let (status, body) = match serde_json::from_slice::<crate::routines::RoutineSpec>(&body_bytes) {
        Ok(spec) => match crate::routines::create(&spec) {
            Ok(created) => (200, serde_json::to_string(&created).unwrap_or_default()),
            Err(e) => (500, serde_json::json!({ "error": e }).to_string()),
        },
        Err(e) => (400, serde_json::json!({ "error": e.to_string() }).to_string()),
    };
    let _ = request.respond(
        tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(json_header),
    );
}

/// `GET /projects` — the project registry (`~/.fleet/projects.json`), after
/// registering every directory the session snapshot saw an agent in.
pub(crate) fn route_projects(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sessions = ctx.snapshot.sessions();
    if let Err(e) =
        crate::project_registry::discover(sessions.iter().map(|s| s.workspace_path.as_str()))
    {
        crate::log_debug(&format!("[projects] discover failed: {e}"));
    }
    let body = serde_json::to_string(&crate::project_registry::list()).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

#[derive(serde::Deserialize)]
struct ArchiveProjectBody {
    path: String,
    archived: bool,
}

/// `POST /projects/archive` — body `{ path, archived }`; answers the updated
/// registry.
pub(crate) fn route_project_archive(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    let (status, body) = match serde_json::from_slice::<ArchiveProjectBody>(&body_bytes) {
        Ok(b) => match crate::project_registry::set_archived(&b.path, b.archived) {
            Ok(list) => (200, serde_json::to_string(&list).unwrap_or_default()),
            Err(e) => (500, serde_json::json!({ "error": e }).to_string()),
        },
        Err(e) => (400, serde_json::json!({ "error": e.to_string() }).to_string()),
    };
    let _ = request.respond(
        tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(json_header),
    );
}
