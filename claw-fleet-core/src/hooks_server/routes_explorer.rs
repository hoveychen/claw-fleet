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
                let ws = decode("ws");
                let known: Vec<String> = sources
                    .iter()
                    .flat_map(|s| s.scan_sessions())
                    .map(|s| s.workspace_path)
                    .collect();
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
                let known: Vec<String> = sources
                    .iter()
                    .flat_map(|s| s.scan_sessions())
                    .map(|s| s.workspace_path)
                    .collect();
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
