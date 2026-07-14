//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_apply_plan_approval_hook(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                match hooks::apply_plan_approval_hook() {
                    Ok(()) => {
                        let _ = request.respond(
                            tiny_http::Response::from_string(r#"{"ok":true}"#)
                                .with_header(json_header),
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

pub(crate) fn route_remove_plan_approval_hook(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                match hooks::remove_plan_approval_hook() {
                    Ok(()) => {
                        let _ = request.respond(
                            tiny_http::Response::from_string(r#"{"ok":true}"#)
                                .with_header(json_header),
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

pub(crate) fn route_plan_approval_pending(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let ids = plan_approval::list_pending_requests();
                let sessions = scan_all_sources(sources);
                let mut requests = Vec::new();
                for id in &ids {
                    if let Some(mut req) = plan_approval::read_request(id) {
                        if let Some(s) = sessions.iter().find(|s| s.id == req.session_id) {
                            if req.workspace_name.is_empty() {
                                req.workspace_name = s.workspace_name.clone();
                            }
                            if req.ai_title.is_none() {
                                req.ai_title = s.ai_title.clone();
                            }
                        }
                        requests.push(req);
                    }
                }
                // Cards whose wait timed out live in the parked store instead of the
                // channel's request dir — the producer that was blocking on them is
                // gone. They stay pending here until the user resolves them.
                for mut req in crate::parked::list_requests::<plan_approval::PlanApprovalRequest>(crate::parked::ParkedKind::PlanApproval) {
                    if let Some(sess) = sessions.iter().find(|s| s.id == req.session_id) {
                        if req.workspace_name.is_empty() {
                            req.workspace_name = sess.workspace_name.clone();
                        }
                        if req.ai_title.is_none() {
                            req.ai_title = sess.ai_title.clone();
                        }
                    }
                    requests.push(req);
                }
                let body = serde_json::to_string(&requests).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_plan_approval_respond(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<plan_approval::PlanApprovalResponse>(&body_bytes) {
                    Ok(resp) => {
                        // A parked card has no producer left polling for a response
                        // file, so `deliver` resumes the session with the answer
                        // instead (or drops the card when the user dismissed it).
                        let outcome =
                            crate::parked::deliver(&resp.id, &resp, false, plan_approval::write_response);
                        match outcome {
                            Ok(()) => {
                                let _ = request.respond(
                                    tiny_http::Response::from_string(r#"{"ok":true}"#)
                                        .with_header(json_header),
                                );
                            }
                            Err(e) => {
                                let body = serde_json::json!({"error": e}).to_string();
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_status_code(if e.contains("no pending request") { 404 } else { 500 })
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
