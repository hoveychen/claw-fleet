//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_skills(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut workspaces: Vec<String> = ctx
                    .sources
                    .iter()
                    .flat_map(|source| source.scan_sessions())
                    .map(|session| session.workspace_path)
                    .filter(|path| !path.is_empty())
                    .collect();
                workspaces.sort();
                workspaces.dedup();
                let items = skills::scan_all_skills_for_workspaces(&workspaces);
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_skill_sync(
    _ctx: &ServeCtx,
    mut request: tiny_http::Request,
    _query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    _path: &str,
) {
    if request.method() == &tiny_http::Method::Get {
        return respond_skill_sync(request, crate::skill_sync::inventory(), json_header);
    }

    #[derive(serde::Deserialize)]
    #[serde(tag = "operation", rename_all = "kebab-case")]
    enum Operation {
        Sync,
        Adopt { path: String },
        Unlink {
            slug: String,
            target: crate::skill_sync::SkillTarget,
        },
    }

    let mut body = String::new();
    let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
    let result = serde_json::from_str::<Operation>(&body)
        .map_err(|error| error.to_string())
        .and_then(|operation| match operation {
            Operation::Sync => crate::skill_sync::sync(true)
                .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string())),
            Operation::Adopt { path } => crate::skill_sync::adopt(std::path::Path::new(&path))
                .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string())),
            Operation::Unlink { slug, target } => crate::skill_sync::unlink(&slug, target)
                .and_then(|value| serde_json::to_value(value).map_err(|error| error.to_string())),
        });
    respond_skill_sync(request, result, json_header);
}

fn respond_skill_sync<T: serde::Serialize>(
    request: tiny_http::Request,
    result: Result<T, String>,
    json_header: tiny_http::Header,
) {
    match result {
        Ok(value) => {
            let body = serde_json::to_string(&value).unwrap_or_else(|_| "null".to_string());
            let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
        }
        Err(error) => {
            let body = serde_json::json!({"error": error}).to_string();
            let _ = request.respond(
                tiny_http::Response::from_string(body)
                    .with_status_code(400)
                    .with_header(json_header),
            );
        }
    }
}

pub(crate) fn route_plugins(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let items = plugins::scan_with_catalog();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_plugins_set_enabled(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                #[derive(serde::Deserialize)]
                struct Body {
                    plugin_id: String,
                    enabled: bool,
                }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<Body, _> = serde_json::from_str(&buf);
                match parsed {
                    Ok(body) => {
                        match crate::claude_cli::set_plugin_enabled(
                            &body.plugin_id,
                            body.enabled,
                        ) {
                            Ok(()) => {
                                let _ = request.respond(
                                    tiny_http::Response::from_string(r#"{"ok":true}"#)
                                        .with_header(json_header),
                                );
                            }
                            Err(e) => {
                                let body =
                                    serde_json::json!({"error": e.to_string()}).to_string();
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

pub(crate) fn route_plugins_marketplaces(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let items =
                    crate::claude_cli::list_marketplaces().unwrap_or_default();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_plugins_marketplaces_add(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                #[derive(serde::Deserialize)]
                struct Body {
                    source: String,
                }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                match serde_json::from_str::<Body>(&buf) {
                    Ok(body) => match crate::claude_cli::add_marketplace(&body.source)
                    {
                        Ok(()) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(r#"{"ok":true}"#)
                                    .with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = serde_json::json!({"error": e.to_string()}).to_string();
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

pub(crate) fn route_plugins_marketplaces_remove(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                #[derive(serde::Deserialize)]
                struct Body {
                    name: String,
                }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                match serde_json::from_str::<Body>(&buf) {
                    Ok(body) => match crate::claude_cli::remove_marketplace(&body.name)
                    {
                        Ok(()) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(r#"{"ok":true}"#)
                                    .with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = serde_json::json!({"error": e.to_string()}).to_string();
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

pub(crate) fn route_plugins_install(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                #[derive(serde::Deserialize)]
                struct Body {
                    plugin_id: String,
                }
                let is_install = request.url().starts_with(crate::routes::PLUGINS_INSTALL);
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<Body, _> = serde_json::from_str(&buf);
                match parsed {
                    Ok(body) => {
                        let result = if is_install {
                            crate::claude_cli::install_plugin(&body.plugin_id)
                        } else {
                            crate::claude_cli::uninstall_plugin(&body.plugin_id)
                        };
                        match result {
                            Ok(()) => {
                                let _ = request.respond(
                                    tiny_http::Response::from_string(r#"{"ok":true}"#)
                                        .with_header(json_header),
                                );
                            }
                            Err(e) => {
                                let body =
                                    serde_json::json!({"error": e.to_string()}).to_string();
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

pub(crate) fn route_skill_history(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                if let Some(source) = find_source_for_path(sources, &file_path) {
                    use crate::skill_history;
                    let main_msgs = source.get_messages(&file_path).unwrap_or_default();
                    let mut out = skill_history::extract_from_messages(&main_msgs, false);

                    let main_path = std::path::Path::new(&file_path);
                    for sub in skill_history::subagent_jsonl_paths(main_path) {
                        let sub_str = sub.to_string_lossy().to_string();
                        let Ok(msgs) = source.get_messages(&sub_str) else { continue };
                        out.extend(skill_history::extract_from_messages(&msgs, true));
                    }
                    skill_history::sort_by_timestamp(&mut out);

                    let body = serde_json::to_string(&out).unwrap_or_default();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body).with_header(json_header),
                    );
                } else {
                    let _ = request.respond(tiny_http::Response::empty(404));
                }
            }

pub(crate) fn route_skill_content(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                match skills::read_skill_file(&file_path) {
                    Ok(content) => {
                        let body = serde_json::to_string(&content).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

pub(crate) fn route_skill_files(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let skill_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                match skills::list_skill_files(&skill_path) {
                    Ok(entries) => {
                        let body = serde_json::to_string(&entries).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

pub(crate) fn route_skill_delete(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                #[derive(serde::Deserialize)]
                struct Body {
                    skill_path: String,
                }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<Body, _> = serde_json::from_str(&buf);
                match parsed {
                    Ok(body) => match skills::delete_skill(&body.skill_path) {
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
