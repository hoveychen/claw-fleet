//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_resume_session(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                match serde_json::from_str::<crate::auto_resume::ResumeSessionRequest>(&buf) {
                    Ok(req) => {
                        match crate::auto_resume::spawn_resume_prompt(
                            &req.session_id,
                            &req.workspace_path,
                            req.prompt.as_deref().unwrap_or("continue"),
                            req.model.as_deref(),
                            req.effort.as_deref(),
                            req.permission_mode.as_deref(),
                        ) {
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

pub(crate) fn route_chat_workspace(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                match crate::chat_workspace::ensure_chat_workspace() {
                    Ok(path) => {
                        let body = serde_json::json!({"path": path}).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
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

pub(crate) fn route_browse_dir(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let path = query
                    .get("path")
                    .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                    .filter(|s| !s.is_empty());
                let known: Vec<String> = sources
                    .iter()
                    .flat_map(|s| s.scan_sessions())
                    .map(|s| s.workspace_path)
                    .collect();
                match crate::workspace_browse::browse_dir(path.as_deref(), &known) {
                    Ok(resp) => {
                        let body = serde_json::to_string(&resp).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = serde_json::json!({ "error": e }).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }

pub(crate) fn route_spawn_session(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                match serde_json::from_str::<crate::session_launch::SpawnSessionRequest>(&buf) {
                    Ok(req) => {
                        match crate::session_launch::spawn_new_session_with_id(
                            &req.workspace_path,
                            &req.prompt,
                            req.model.as_deref(),
                            req.effort.as_deref(),
                            req.permission_mode.as_deref(),
                            req.session_id.as_deref(),
                        ) {
                            Ok(resp) => {
                                let body =
                                    serde_json::to_string(&resp).unwrap_or_default();
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
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
