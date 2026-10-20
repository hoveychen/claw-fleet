//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_tasks_accept(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    let (status, body) =
        match serde_json::from_slice::<crate::task_model::AcceptTasksRequest>(&body_bytes) {
            Ok(req) => {
                let accepted = crate::task_model::accept_tasks(&req.session_ids);
                (200, serde_json::json!({ "ok": true, "accepted": accepted }))
            }
            Err(e) => (400, serde_json::json!({ "error": format!("bad request: {e}") })),
        };
    let _ = request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(json_header),
    );
}

/// The profile of the task a session belongs to: see
/// `task_profile::view_for_session`.
pub(crate) fn route_task_profile_get(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    let (status, body) =
        match serde_json::from_slice::<crate::task_profile::TaskProfileQuery>(&body_bytes) {
            Ok(req) => (
                200,
                serde_json::to_value(crate::task_profile::view_for_session(&req.session_id))
                    .unwrap_or_default(),
            ),
            Err(e) => (400, serde_json::json!({ "error": format!("bad request: {e}") })),
        };
    let _ = request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(json_header),
    );
}

/// The boss rewriting a task's type / goal: see `task_profile::set_by_boss`.
/// A goal change without a reason is a 400 carrying the refusal text.
pub(crate) fn route_task_profile_set(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    let (status, body) =
        match serde_json::from_slice::<crate::task_profile::SetTaskProfileRequest>(&body_bytes) {
            Ok(req) => match crate::task_profile::set_by_boss(&req) {
                Ok(p) => (200, serde_json::to_value(p).unwrap_or_default()),
                Err(e) => (400, serde_json::json!({ "error": e })),
            },
            Err(e) => (400, serde_json::json!({ "error": format!("bad request: {e}") })),
        };
    let _ = request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(json_header),
    );
}

/// Undo an acceptance / a terminal button: see `task_model::reopen_tasks`.
pub(crate) fn route_tasks_reopen(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    let (status, body) =
        match serde_json::from_slice::<crate::task_model::AcceptTasksRequest>(&body_bytes) {
            Ok(req) => {
                let reopened = crate::task_model::reopen_tasks(&req.session_ids);
                (200, serde_json::json!({ "ok": true, "reopened": reopened }))
            }
            Err(e) => (400, serde_json::json!({ "error": format!("bad request: {e}") })),
        };
    let _ = request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(json_header),
    );
}

/// A coordinator's sub-tasks, each probed and classified for recovery: see
/// `subtask_recovery::children_of`.
pub(crate) fn route_subtasks_recovery(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    let (status, body) =
        match serde_json::from_slice::<crate::subtask_recovery::SubtaskListRequest>(&body_bytes) {
            Ok(req) => (
                200,
                serde_json::to_value(crate::subtask_recovery::children_of(&req.session_id))
                    .unwrap_or_default(),
            ),
            Err(e) => (400, serde_json::json!({ "error": format!("bad request: {e}") })),
        };
    let _ = request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(json_header),
    );
}

/// Resume the named sub-tasks, one guarded `resume_child` each; answers the
/// per-id outcomes (refusals included) rather than failing the batch.
pub(crate) fn route_subtasks_resume(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    let (status, body) = match serde_json::from_slice::<
        crate::subtask_recovery::ResumeChildrenRequest,
    >(&body_bytes)
    {
        Ok(req) => (
            200,
            serde_json::to_value(crate::subtask_recovery::resume_children(
                &req.session_ids,
                req.prompt.as_deref(),
            ))
            .unwrap_or_default(),
        ),
        Err(e) => (400, serde_json::json!({ "error": format!("bad request: {e}") })),
    };
    let _ = request.respond(
        tiny_http::Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(json_header),
    );
}

pub(crate) fn route_session_mark(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    match serde_json::from_slice::<crate::session_mark::SetSessionMarkRequest>(&body_bytes) {
        Ok(req_body) => {
            match crate::session_mark::set_mark(
                &req_body.session_id,
                &req_body.workspace_path,
                req_body.mark,
            ) {
                Ok(()) => {
                    let _ = request.respond(
                        tiny_http::Response::from_string(r#"{"ok":true}"#).with_header(json_header),
                    );
                }
                Err(e) => {
                    let body = serde_json::json!({"error": e}).to_string();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_status_code(500)
                            .with_header(json_header),
                    );
                }
            }
        }
        Err(e) => {
            let body = serde_json::json!({"error": e.to_string()}).to_string();
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(400)
                    .with_header(json_header),
            );
        }
    }
}

pub(crate) fn route_session_title(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    match serde_json::from_slice::<crate::session_title::SetSessionTitleRequest>(&body_bytes) {
        Ok(req_body) => {
            match crate::session_title::set_title(
                &req_body.session_id,
                &req_body.workspace_path,
                req_body.title,
            ) {
                Ok(()) => {
                    let _ = request.respond(
                        tiny_http::Response::from_string(r#"{"ok":true}"#).with_header(json_header),
                    );
                }
                Err(e) => {
                    let body = serde_json::json!({"error": e}).to_string();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_status_code(500)
                            .with_header(json_header),
                    );
                }
            }
        }
        Err(e) => {
            let body = serde_json::json!({"error": e.to_string()}).to_string();
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(400)
                    .with_header(json_header),
            );
        }
    }
}
