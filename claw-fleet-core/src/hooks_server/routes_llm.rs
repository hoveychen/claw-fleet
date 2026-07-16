//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_llm_providers(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let infos = llm_provider::all_provider_infos();
                let body = serde_json::to_string(&infos).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_llm_config_get(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let llm_config = ctx.llm_config.clone();

                let cfg = llm_config.lock().unwrap().clone();
                let body = serde_json::to_string(&cfg).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_llm_config_1(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let llm_config = ctx.llm_config.clone();

                // POST: update config
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<LlmConfig>(&body_bytes) {
                    Ok(new_cfg) => {
                        // Mirror into the process-wide slot so the mobile
                        // relay's `guard_analyze` follows the same provider.
                        crate::llm_provider::set_shared_config(new_cfg.clone());
                        *llm_config.lock().unwrap() = new_cfg;
                        let _ = request.respond(
                            tiny_http::Response::from_string("{}").with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = format!(r#"{{"error":"invalid config: {}"}}"#, e.to_string().replace('"', "'"));
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }

pub(crate) fn route_fleet_llm_usage_daily(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let from_ms = query
                    .get("from_ms")
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(0);
                let to_ms = query
                    .get("to_ms")
                    .and_then(|s| s.parse::<u64>().ok())
                    .unwrap_or(u64::MAX);
                let buckets = crate::llm_usage::list_usage_daily_buckets(from_ms, to_ms);
                let body = serde_json::to_string(&buckets).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_usage_history(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let from_ms = query
                    .get("from_ms")
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(0);
                let to_ms = query
                    .get("to_ms")
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(i64::MAX);
                let points = crate::account::load_usage_history(from_ms, to_ms);
                let body = serde_json::to_string(&points).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_codex_usage_history(
    _ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    _path: &str,
) {
    let from_ms = query
        .get("from_ms")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let to_ms = query
        .get("to_ms")
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(i64::MAX);
    let points = crate::codex_usage_history::load_codex_usage_history(from_ms, to_ms);
    let body = serde_json::to_string(&points).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_analyze(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let llm_config = ctx.llm_config.clone();

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<claude_analyze::AnalyzeRequest>(&body_bytes) {
                    Ok(req) => {
                        let cfg = llm_config.lock().unwrap().clone();
                        let provider = llm_provider::resolve_provider(&cfg.provider);
                        let result = provider.as_ref().and_then(|p| {
                            claude_analyze::analyze_session_outcome(
                                p.as_ref(),
                                &cfg.fast_model,
                                &req.last_text,
                                &req.locale,
                                &req.session_id,
                                &req.user_title,
                            )
                        });
                        match result {
                            Some(analysis) => {
                                let body = serde_json::to_string(&analysis).unwrap_or_default();
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_header(json_header),
                                );
                            }
                            None => {
                                let body = r#"{"error":"LLM analysis unavailable"}"#;
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_status_code(503)
                                        .with_header(json_header),
                                );
                            }
                        }
                    }
                    Err(e) => {
                        let body = format!(
                            r#"{{"error":"invalid request: {}"}}"#,
                            e.to_string().replace('"', "'")
                        );
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }
