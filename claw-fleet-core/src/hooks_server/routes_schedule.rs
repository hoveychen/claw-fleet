//! Route handlers for `fleet serve`'s future-task endpoints — agent loops
//! (`fleet loop`) and one-shot schedules (`fleet schedule`). These read the
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
