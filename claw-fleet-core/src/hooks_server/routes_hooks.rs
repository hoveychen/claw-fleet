//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_hooks_plan(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let plan = hooks::plan_hook_setup();
                let body = serde_json::to_string(&plan).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_apply_hooks(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                match hooks::apply_hook_setup() {
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

pub(crate) fn route_sources_config(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let info = agent_source::get_sources_config_local();
                let body = serde_json::to_string(&info).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

/// `GET /codex_profiles` — the probe host's Codex profile-v2 files, so a
/// desktop driving a remote workspace offers the models that host can actually
/// resolve rather than its own.
pub(crate) fn route_codex_profiles(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let profiles = crate::codex_launch::list_codex_profiles();
    let body = serde_json::to_string(&profiles).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_set_source_enabled(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let name = query.get("name").cloned().unwrap_or_default();
                let enabled: bool = query.get("enabled").map(|s| s == "true").unwrap_or(false);
                if name.is_empty() {
                    let _ = request.respond(
                        tiny_http::Response::from_string(r#"{"error":"missing name param"}"#)
                            .with_status_code(400)
                            .with_header(json_header),
                    );
                } else {
                    match agent_source::set_source_enabled_local(&name, enabled) {
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
            }

pub(crate) fn route_list_claude_binaries(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let bins = crate::claude_binary::discover();
                let body = serde_json::to_string(&bins).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_claude_binary_override(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                if request.method() == &tiny_http::Method::Post {
                    #[derive(serde::Deserialize)]
                    struct Body { path: Option<String> }
                    let mut buf = String::new();
                    let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                    let parsed: Result<Body, _> = serde_json::from_str(&buf);
                    let path = parsed.ok().and_then(|b| b.path);
                    let cleaned = path.and_then(|p| {
                        let t = p.trim().to_string();
                        if t.is_empty() { None } else { Some(t) }
                    });
                    let cfg = crate::claude_binary::ClaudeBinaryConfig { override_path: cleaned };
                    match cfg.save() {
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
                } else {
                    let cfg = crate::claude_binary::ClaudeBinaryConfig::load();
                    let body = serde_json::json!({ "path": cfg.override_path }).to_string();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body).with_header(json_header),
                    );
                }
            }

pub(crate) fn route_remove_hooks(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                match hooks::remove_fleet_hooks() {
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
