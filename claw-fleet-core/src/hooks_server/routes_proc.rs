//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

/// `GET /host_features` — the launch-time feature flags of *this* host.
///
/// Lives beside the proc routes because the only flag so far gates them: a
/// browser build that shows a 终端 page against a backend started without
/// `FLEET_TERMINAL` would offer a shell it cannot open. The answer comes from
/// the same `feature_flags::terminal_enabled()` that `proc_runner` enforces, so
/// UI and enforcement cannot disagree.
pub(crate) fn route_host_features(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let body = serde_json::to_string(&crate::feature_flags::host_features()).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

/// `GET /host_identity` —— 这台主机的展示身份(主机名 + 平台)。
///
/// 和上面那条一样是「一次性、启动后就不变」的元信息,所以放在一起;但它不 gate
/// 任何面 —— 客户端拿不到时退回一个平台名,而不是把某个入口藏起来。
pub(crate) fn route_host_identity(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let body = serde_json::to_string(&crate::host_identity::host_identity()).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_procs(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let body =
                    serde_json::to_string(&crate::proc_runner::list_procs()).unwrap_or_default();
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_header(json_header));
            }

pub(crate) fn route_proc_run(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::proc_runner::SpawnProcRequest>(&body_bytes) {
                    Ok(req_body) => {
                        let host_exe = std::env::current_exe()
                            .map_err(|e| format!("cannot locate fleet binary: {e}"));
                        let result = host_exe.and_then(|exe| {
                            crate::proc_runner::spawn_proc(
                                &exe,
                                &req_body.workspace_path,
                                &req_body.command,
                                req_body.cols,
                                req_body.rows,
                            )
                        });
                        match result {
                            Ok(rec) => {
                                let body = serde_json::to_string(&rec).unwrap_or_default();
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

pub(crate) fn route_proc_output(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let id = query.get("id").map(|s| s.as_str()).unwrap_or("");
                let offset: Option<u64> = query.get("offset").and_then(|s| s.parse().ok());
                match crate::proc_runner::proc_output(id, offset) {
                    Ok(chunk) => {
                        let body = serde_json::to_string(&chunk).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = serde_json::json!({"error": e}).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(404)
                                .with_header(json_header),
                        );
                    }
                }
            }

pub(crate) fn route_proc_input(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::proc_runner::ProcInputRequest>(&body_bytes) {
                    Ok(req_body) => {
                        match crate::proc_runner::proc_input(&req_body.id, &req_body.data_b64) {
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

pub(crate) fn route_proc_resize(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::proc_runner::ProcResizeRequest>(&body_bytes) {
                    Ok(req_body) => {
                        match crate::proc_runner::proc_resize(
                            &req_body.id,
                            req_body.cols,
                            req_body.rows,
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

pub(crate) fn route_proc_kill(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let id = query.get("id").map(|s| s.as_str()).unwrap_or("");
                let force = query.get("force").map(|s| s == "true").unwrap_or(false);
                match crate::proc_runner::kill_proc(id, force) {
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

pub(crate) fn route_proc_clear(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::proc_runner::ClearProcRequest>(&body_bytes) {
                    Ok(req_body) => {
                        let result = match req_body.id {
                            Some(id) => crate::proc_runner::clear_proc(&id).map(|()| 1u32),
                            None => crate::proc_runner::clear_finished_procs(
                                req_body.workspace_path.as_deref(),
                            ),
                        };
                        match result {
                            Ok(cleared) => {
                                let body = serde_json::json!({"cleared": cleared}).to_string();
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
