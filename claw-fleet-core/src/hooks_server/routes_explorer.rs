//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_workflow_trees(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                // Claude Code Workflow runs for a session. `path` is the parent
                // session's .jsonl transcript path; discovery reads the sibling
                // `<sid>/subagents/workflows/wf_*/` dirs server-side so remote
                // clients see runs that live on the remote host.
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let trees = crate::workflow::discover_workflow_trees(std::path::Path::new(
                    &file_path,
                ));
                let body = serde_json::to_string(&trees).unwrap_or_default();
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_header(json_header));
            }

pub(crate) fn route_token_breakdown(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let project_root = query.get("project_root").map(|s| {
                    percent_decode_str(s).decode_utf8_lossy().to_string()
                });
                let main_path = std::path::Path::new(&file_path);
                let project_path = project_root.as_deref().map(std::path::Path::new);
                match crate::token_analysis::aggregate_task(main_path, project_path) {
                    Ok(t) => {
                        let body = serde_json::to_string(&t).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let _ = request.respond(
                            tiny_http::Response::from_string(format!("{{\"error\":{}}}", serde_json::to_string(&e).unwrap_or_default()))
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                    }
                }
            }

pub(crate) fn route_codex_token_breakdown(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
    let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
    match crate::codex_source::codex_token_breakdown(&file_path) {
        Ok(t) => {
            let body = serde_json::to_string(&t).unwrap_or_default();
            let _ = request
                .respond(tiny_http::Response::from_string(body).with_header(json_header));
        }
        Err(e) => {
            let _ = request.respond(
                tiny_http::Response::from_string(format!(
                    "{{\"error\":{}}}",
                    serde_json::to_string(&e).unwrap_or_default()
                ))
                .with_status_code(500)
                .with_header(json_header),
            );
        }
    }
}

pub(crate) fn route_dsh_token_breakdown(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    // `path` here is a `dsh://<session-id>` URI, not a filesystem path — dsh
    // sessions have no file. The param keeps the name the other breakdown
    // routes use so the remote client's encoder stays shared.
    let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
    let uri = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
    match crate::dsh_source::dsh_token_breakdown(&uri) {
        Ok(t) => {
            let body = serde_json::to_string(&t).unwrap_or_default();
            let _ = request
                .respond(tiny_http::Response::from_string(body).with_header(json_header));
        }
        Err(e) => {
            let _ = request.respond(
                tiny_http::Response::from_string(format!(
                    "{{\"error\":{}}}",
                    serde_json::to_string(&e).unwrap_or_default()
                ))
                .with_status_code(500)
                .with_header(json_header),
            );
        }
    }
}

/// Real per-session spend for a `dsh://` URI. Same `path` param convention as
/// [`route_dsh_token_breakdown`] — the value is a URI, not a file.
pub(crate) fn route_dsh_session_cost(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let _ = (ctx, path);
    let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
    let uri = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
    match crate::dsh_cost::dsh_session_cost(&uri) {
        Ok(c) => {
            let body = serde_json::to_string(&c).unwrap_or_default();
            let _ = request
                .respond(tiny_http::Response::from_string(body).with_header(json_header));
        }
        Err(e) => {
            let _ = request.respond(
                tiny_http::Response::from_string(format!(
                    "{{\"error\":{}}}",
                    serde_json::to_string(&e).unwrap_or_default()
                ))
                .with_status_code(500)
                .with_header(json_header),
            );
        }
    }
}

pub(crate) fn route_explorer_roots(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let decode = |key: &str| {
                    query
                        .get(key)
                        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                        .unwrap_or_default()
                };
                // The out-of-workspace read has no workspace to validate, so it
                // answers before the session scan every gated arm below needs.
                if path == crate::routes::EXPLORER_EXTERNAL_FILE {
                    respond_explorer_json(
                        request,
                        json_header,
                        crate::file_explorer::read_external_file(&decode("path"))
                            .map(|c| serde_json::to_string(&c).unwrap_or_default()),
                    );
                    return;
                }

                let ws = decode("ws");
                let sessions: Vec<String> = sources
                    .iter()
                    .flat_map(|s| s.scan_sessions())
                    .map(|s| s.workspace_path)
                    .collect();
                let known = crate::file_explorer::browsable_workspaces(&sessions);
                let result: Result<String, String> = match path {
                    crate::routes::EXPLORER_ROOTS => crate::file_explorer::list_roots(&ws, &known)
                        .map(|r| serde_json::to_string(&r).unwrap_or_default()),
                    crate::routes::EXPLORER_DIR => {
                        let show_ignored = query.get("ignored").map(|s| s == "true").unwrap_or(false);
                        crate::file_explorer::list_dir(
                            &ws,
                            &decode("root"),
                            &decode("rel"),
                            show_ignored,
                            &known,
                        )
                        .map(|e| serde_json::to_string(&e).unwrap_or_default())
                    }
                    crate::routes::SCRATCHPAD_DIR => crate::file_explorer::list_scratchpad_dir(
                        &ws,
                        &decode("session"),
                        &decode("rel"),
                        &known,
                    )
                    .map(|e| serde_json::to_string(&e).unwrap_or_default()),
                    crate::routes::SCRATCHPAD_FILE => crate::file_explorer::read_scratchpad_file(
                        &ws,
                        &decode("session"),
                        &decode("rel"),
                        &known,
                    )
                    .map(|c| serde_json::to_string(&c).unwrap_or_default()),
                    _ => crate::file_explorer::read_file(&ws, &decode("root"), &decode("rel"), &known)
                        .map(|c| serde_json::to_string(&c).unwrap_or_default()),
                };
                respond_explorer_json(request, json_header, result);
            }

/// Serialised body → 200, explorer error → 400 with `{"error": …}`. Shared by
/// every arm of `route_explorer_roots`, including the early-returning one.
fn respond_explorer_json(
    request: tiny_http::Request,
    json_header: tiny_http::Header,
    result: Result<String, String>,
) {
    match result {
        Ok(body) => {
            let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
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

/// POST `/git_clone` with `{"url": "...", "dest": "..."}`.
///
/// The only git route with no `ws` / `root`: a clone destination is by
/// definition not a known workspace yet, so there is nothing to validate it
/// against. `git_ops::git_clone` owns the structural guard rails instead
/// (absolute dest, existing parent, empty-or-absent dest), and this route adds
/// nothing on top — a probe that serves the explorer already lets the caller
/// pick any cwd for a new session.
///
/// On success the destination is registered in `browse_paths`, mirroring
/// `LocalBackend::git_clone`: the freshly cloned repo has no sessions, so the
/// explorer would otherwise refuse to open the very directory it just created.
pub(crate) fn route_git_clone(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        url: String,
        dest: String,
    }
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    let result = serde_json::from_str::<Req>(&buf)
        .map_err(|e| e.to_string())
        .and_then(|req| {
            let out = crate::git_ops::git_clone(&req.url, &req.dest)?;
            if let Err(e) = crate::browse_paths::add(&req.dest) {
                crate::log_debug(&format!(
                    "[serve] clone succeeded but registering {} as browsable failed: {e}",
                    req.dest
                ));
            }
            Ok(out)
        })
        .map(|r| serde_json::to_string(&r).unwrap_or_default());
    match result {
        Ok(body) => {
            let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
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

/// POST `/git_clone_stream` with `{"url": "...", "dest": "..."}` — start the
/// clone as a detached proc and return its [`crate::proc_runner::ProcRecord`],
/// which the client then tails through `/proc_output`.
///
/// Same guard rails as `/git_clone` (they live in `prepare_clone`), plus the
/// shell quoting that path doesn't need: this one goes through the proc host's
/// `$SHELL -c`, so the url is quoted rather than passed as argv.
///
/// Unlike the blocking route, registering the destination in `browse_paths`
/// can't happen here — the clone has only just started. The client does it once
/// the proc exits 0.
pub(crate) fn route_git_clone_stream(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        url: String,
        dest: String,
    }
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    let result = serde_json::from_str::<Req>(&buf)
        .map_err(|e| e.to_string())
        .and_then(|req| {
            let prepared = crate::git_ops::prepare_clone(&req.url, &req.dest)?;
            let exe = std::env::current_exe()
                .map_err(|e| format!("cannot locate fleet binary: {e}"))?;
            crate::proc_runner::spawn_proc(
                &exe,
                &prepared.parent.to_string_lossy(),
                &prepared.command,
                crate::git_ops::CLONE_PTY_COLS,
                crate::git_ops::CLONE_PTY_ROWS,
            )
        })
        .map(|rec| serde_json::to_string(&rec).unwrap_or_default());
    match result {
        Ok(body) => {
            let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
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

/// GET `/browse_paths` — the directories the user registered on this host.
pub(crate) fn route_browse_paths(
    _ctx: &ServeCtx,
    request: tiny_http::Request,
    json_header: tiny_http::Header,
) {
    let body = serde_json::to_string(&crate::browse_paths::list()).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

/// POST `/browse_paths/add` | `/browse_paths/remove` with `{"path": "..."}`.
///
/// This is the one surface that widens what the explorer will read, so it is
/// deliberately a separate, explicit call rather than a per-read argument: the
/// probe token already lets a caller start a session in any directory, but a
/// single mis-issued explorer request must not be able to name an arbitrary
/// path. Both respond with the updated list.
pub(crate) fn route_browse_paths_mutate(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    json_header: tiny_http::Header,
    path: &str,
) {
    #[derive(serde::Deserialize)]
    struct Req {
        path: String,
    }
    let mut buf = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
    let adding = path == crate::routes::BROWSE_PATHS_ADD;
    let result = serde_json::from_str::<Req>(&buf)
        .map_err(|e| e.to_string())
        .and_then(|req| {
            if adding {
                crate::browse_paths::add(&req.path)
            } else {
                crate::browse_paths::remove(&req.path)
            }
        })
        .map(|list| serde_json::to_string(&list).unwrap_or_default());
    match result {
        Ok(body) => {
            let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
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

pub(crate) fn route_git_status(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let decode = |key: &str| {
                    query
                        .get(key)
                        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                        .unwrap_or_default()
                };
                let ws = decode("ws");
                let root = decode("root");
                let sessions: Vec<String> = sources
                    .iter()
                    .flat_map(|s| s.scan_sessions())
                    .map(|s| s.workspace_path)
                    .collect();
                let known = crate::file_explorer::browsable_workspaces(&sessions);
                let result: Result<String, String> = match path {
                    crate::routes::GIT_STATUS => crate::git_ops::git_status(&ws, &root, &known)
                        .map(|s| serde_json::to_string(&s).unwrap_or_default()),
                    crate::routes::GIT_PUSH => crate::git_ops::git_push(&ws, &root, &known)
                        .map(|r| serde_json::to_string(&r).unwrap_or_default()),
                    _ => crate::git_ops::git_pull(&ws, &root, &known)
                        .map(|r| serde_json::to_string(&r).unwrap_or_default()),
                };
                match result {
                    Ok(body) => {
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
