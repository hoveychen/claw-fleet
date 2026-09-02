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
                        // A "done" task resumed by a remote client (RemoteBackend)
                        // is active again — drop the done mark on the host where it
                        // lives so it re-surfaces as needs-review.
                        crate::session_mark::clear_done_on_resume(&req.session_id, &req.workspace_path);
                        // Same as the desktop resume: a retry clears the stale
                        // remote-disconnect verdict on the host where it lives.
                        crate::remote_disconnect::clear(&req.session_id);
                        crate::mirror_guard::clear(&req.session_id);
                        // Route by source: codex sessions resume via
                        // `codex exec resume`, claude via `claude --resume`.
                        // Manual resume is untracked → no-op on_exit box.
                        match crate::agent_source::resume_session(
                            &req.agent_source,
                            &crate::agent_source::ResumeSpec {
                                session_id: req.session_id.clone(),
                                workspace_path: req.workspace_path.clone(),
                                prompt: req.prompt.clone().unwrap_or_else(|| "continue".to_string()),
                                model: req.model.clone(),
                                effort: req.effort.clone(),
                                permission_mode: req.permission_mode.clone(),
                            images: Vec::new(),
                            },
                            Box::new(|_| {}),
                        ) {
                            Ok(()) => {
                                // Same reason as the spawn route: a resumed
                                // session changes the roster, and only a rescan
                                // puts that in the snapshot.
                                ctx.snapshot.invalidate();
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

pub(crate) fn route_enqueue_message(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    match serde_json::from_str::<crate::pending_message::EnqueueMessageRequest>(&buf) {
        Ok(req) => match crate::pending_message::enqueue(
            &req.session_id,
            &req.workspace_path,
            &req.text,
        ) {
            Ok(()) => {
                let _ = request.respond(
                    tiny_http::Response::from_string(r#"{"ok":true}"#).with_header(json_header),
                );
            }
            Err(e) => {
                let body = serde_json::json!({ "error": e }).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(500)
                        .with_header(json_header),
                );
            }
        },
        Err(e) => {
            let body = serde_json::json!({ "error": e.to_string() }).to_string();
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(400)
                    .with_header(json_header),
            );
        }
    }
}

pub(crate) fn route_cancel_pending_message(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    _query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    _path: &str,
) {
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    match serde_json::from_str::<crate::pending_message::CancelMessageRequest>(&buf) {
        Ok(req) => match crate::pending_message::remove_at(&req.session_id, req.index) {
            Ok(()) => {
                let _ = request.respond(
                    tiny_http::Response::from_string(r#"{"ok":true}"#).with_header(json_header),
                );
            }
            Err(e) => {
                let body = serde_json::json!({ "error": e }).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(500)
                        .with_header(json_header),
                );
            }
        },
        Err(e) => {
            let body = serde_json::json!({ "error": e.to_string() }).to_string();
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

/// GET `/remote_browse_dir?target=<ssh target>&path=<dir>` — the listing is of
/// a host one ssh hop past this one, so the ssh originates here (this process
/// is where sessions spawn and where the keys live).
pub(crate) fn route_remote_browse_dir(
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
) {
    let decode = |k: &str| {
        query
            .get(k)
            .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
            .filter(|s| !s.is_empty())
    };
    let Some(target) = decode("target") else {
        let body = serde_json::json!({ "error": "target is required" }).to_string();
        let _ = request.respond(
            tiny_http::Response::from_string(body).with_status_code(400).with_header(json_header),
        );
        return;
    };
    match crate::remote_host::browse_remote_dir(&target, decode("path").as_deref()) {
        Ok(resp) => {
            let body = serde_json::to_string(&resp).unwrap_or_default();
            let _ =
                request.respond(tiny_http::Response::from_string(body).with_header(json_header));
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

/// GET `/remote_host_health?target=<ssh target>`. Only a missing `target` is a
/// 400: an unreachable host is a 200 carrying `sshOk: false`, because the
/// caller is a status badge and needs the reason, not an HTTP error.
pub(crate) fn route_remote_host_health(
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
) {
    let target = query
        .get("target")
        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
        .filter(|s| !s.is_empty());
    let Some(target) = target else {
        let body = serde_json::json!({ "error": "target is required" }).to_string();
        let _ = request.respond(
            tiny_http::Response::from_string(body).with_status_code(400).with_header(json_header),
        );
        return;
    };
    let body =
        serde_json::to_string(&crate::remote_host::host_health(&target)).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_create_dir(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let body: serde_json::Value = serde_json::from_str(&buf).unwrap_or_default();
                let parent = body
                    .get("path")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty());
                let name = body.get("name").and_then(|v| v.as_str()).unwrap_or_default();
                let known: Vec<String> = sources
                    .iter()
                    .flat_map(|s| s.scan_sessions())
                    .map(|s| s.workspace_path)
                    .collect();
                match crate::workspace_browse::create_dir(parent, name, &known) {
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
                        // Route by tool ("claude" default / "codex"); the spec
                        // preserves the caller-preassigned session_id for the
                        // Claude idempotency path (Codex ignores it).
                        let spec = crate::agent_source::SpawnSpec {
                            workspace_path: req.workspace_path.clone(),
                            prompt: req.prompt.clone(),
                            model: req.model.clone(),
                            effort: req.effort.clone(),
                            permission_mode: req.permission_mode.clone(),
                            session_id: req.session_id.clone(),
                            entrypoint: String::new(),
                        images: Vec::new(),
                        };
                        match crate::agent_source::spawn_session(
                            req.tool.as_deref().unwrap_or("claude"),
                            &spec,
                        ) {
                            Ok(resp) => {
                                // The roster the routes serve is a snapshot; without
                                // this the caller's own new session would not appear
                                // until the next ticker refresh.
                                ctx.snapshot.invalidate();
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

/// GET the remote-workspace registry (`~/.fleet/remote-workspaces.json`).
/// POST `/remote_create_dir` `{target, path, name}` — mkdir one level on an
/// rca executor host and answer with the new directory's listing.
pub(crate) fn route_remote_create_dir(
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    let body: serde_json::Value = serde_json::from_str(&buf).unwrap_or_default();
    let str_of = |k: &str| body.get(k).and_then(|v| v.as_str()).filter(|s| !s.is_empty());
    let (out, code) = match (str_of("target"), str_of("name")) {
        (Some(target), Some(name)) => {
            match crate::remote_host::create_remote_dir(target, str_of("path"), name) {
                Ok(resp) => (serde_json::to_string(&resp).unwrap_or_default(), 200),
                Err(e) => (serde_json::json!({ "error": e }).to_string(), 400),
            }
        }
        _ => (serde_json::json!({ "error": "target and name are required" }).to_string(), 400),
    };
    let _ = request.respond(
        tiny_http::Response::from_string(out).with_status_code(code).with_header(json_header),
    );
}

/// GET `/ssh_hosts` — the backend host's ssh host book.
pub(crate) fn route_ssh_hosts(request: tiny_http::Request, json_header: tiny_http::Header) {
    let body =
        serde_json::to_string(&crate::remote_host::load_hosts()).unwrap_or_else(|_| "[]".into());
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

/// POST a [`crate::remote_host::SshHost`]; responds with the updated book.
pub(crate) fn route_ssh_hosts_upsert(
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    let (body, code) = match serde_json::from_str::<crate::remote_host::SshHost>(&buf) {
        Ok(host) => match crate::remote_host::upsert_host(host) {
            Ok(book) => (serde_json::to_string(&book).unwrap_or_else(|_| "[]".into()), 200),
            Err(e) => (serde_json::json!({ "error": e }).to_string(), 500),
        },
        Err(e) => (serde_json::json!({ "error": e.to_string() }).to_string(), 400),
    };
    let _ = request.respond(
        tiny_http::Response::from_string(body).with_status_code(code).with_header(json_header),
    );
}

/// POST `{id}`; responds with the updated book.
pub(crate) fn route_ssh_hosts_remove(
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    let id = serde_json::from_str::<serde_json::Value>(&buf)
        .ok()
        .and_then(|v| v.get("id").and_then(|s| s.as_str()).map(str::to_string))
        .filter(|s| !s.is_empty());
    let (body, code) = match id {
        Some(id) => match crate::remote_host::remove_host(&id) {
            Ok(book) => (serde_json::to_string(&book).unwrap_or_else(|_| "[]".into()), 200),
            Err(e) => (serde_json::json!({ "error": e }).to_string(), 500),
        },
        None => (serde_json::json!({ "error": "id is required" }).to_string(), 400),
    };
    let _ = request.respond(
        tiny_http::Response::from_string(body).with_status_code(code).with_header(json_header),
    );
}

pub(crate) fn route_remote_workspaces(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let cfg = crate::remote_workspace::load();
    let body = serde_json::to_string(&cfg).unwrap_or_else(|_| "{}".to_string());
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

/// POST a [`crate::remote_workspace::RemoteWorkspace`] to register/update it.
/// Responds with the full updated registry.
pub(crate) fn route_remote_workspaces_upsert(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    match serde_json::from_str::<crate::remote_workspace::RemoteWorkspace>(&buf) {
        Ok(entry) => match crate::remote_workspace::upsert(entry) {
            Ok(cfg) => {
                let body = serde_json::to_string(&cfg).unwrap_or_else(|_| "{}".to_string());
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_header(json_header));
            }
            Err(e) => {
                let body = serde_json::json!({"error": e}).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(500)
                        .with_header(json_header),
                );
            }
        },
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

/// POST `{"path": "..."}` to remove a remote workspace. Responds with the
/// full updated registry.
pub(crate) fn route_remote_workspaces_remove(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    let ws_path = serde_json::from_str::<serde_json::Value>(&buf)
        .ok()
        .and_then(|v| v.get("path").and_then(|s| s.as_str()).map(|s| s.to_string()));
    match ws_path {
        Some(ws_path) => match crate::remote_workspace::remove(&ws_path) {
            Ok(cfg) => {
                let body = serde_json::to_string(&cfg).unwrap_or_else(|_| "{}".to_string());
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_header(json_header));
            }
            Err(e) => {
                let body = serde_json::json!({"error": e}).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(500)
                        .with_header(json_header),
                );
            }
        },
        None => {
            let _ = request.respond(
                tiny_http::Response::from_string(r#"{"error":"missing path"}"#)
                    .with_status_code(400)
                    .with_header(json_header),
            );
        }
    }
}
