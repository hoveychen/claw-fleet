//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_apply_guard_hook(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                match hooks::apply_guard_hook() {
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

pub(crate) fn route_remove_guard_hook(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                match hooks::remove_guard_hook() {
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

pub(crate) fn route_guard_pending(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let ids = guard::list_pending_requests();
                let sessions = ctx.snapshot.sessions();
                let mut requests = Vec::new();
                for id in &ids {
                    if let Some(mut req) = guard::read_request(id) {
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
                let body = serde_json::to_string(&requests).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_guard_respond(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<guard::GuardRespondPayload>(&body_bytes) {
                    Ok(payload) => {
                        // If the user clicked "Always allow", persist the rule
                        // before writing the response file so a subsequent
                        // `fleet guard` invocation already sees the rule.
                        let allow = matches!(payload.decision, guard::GuardDecision::Allow);
                        if allow {
                            if let Some(rule) = payload.always_allow.as_ref() {
                                if !rule.prefix.trim().is_empty() {
                                    audit::add_guard_allow_rule(
                                        rule.prefix.clone(),
                                        rule.source_tag.clone(),
                                    );
                                }
                            }
                        }
                        let resp = guard::GuardResponse {
                            id: payload.id.clone(),
                            decision: payload.decision.clone(),
                            reason: payload.reason.clone(),
                        };
                        match guard::write_response(&resp) {
                            Ok(()) => {
                                // Don't cleanup here — the `fleet guard` CLI polls
                                // for the response file and does cleanup itself.
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

pub(crate) fn route_guard_analyze(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let llm_config = ctx.llm_config.clone();

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct AnalyzeReq { command: String, context: String, lang: String }
                match serde_json::from_slice::<AnalyzeReq>(&body_bytes) {
                    Ok(req) => {
                        let risk_tags = audit::classify_bash_command_pub(&req.command)
                            .map(|(_, tags)| tags)
                            .unwrap_or_default();
                        let prompt = guard::build_analysis_prompt(
                            &req.command, &risk_tags, &req.context, &req.lang,
                        );
                        let cfg = llm_config.lock().unwrap().clone();
                        let result = llm_provider::complete_routed(
                            &cfg,
                            llm_provider::ModelSlot::Fast,
                            &prompt,
                            std::time::Duration::from_secs(30),
                            crate::llm_usage::SCENARIO_GUARD_COMMAND,
                        );
                        match result {
                            Some(analysis) => {
                                let body = serde_json::json!({"analysis": analysis}).to_string();
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_header(json_header),
                                );
                            }
                            None => {
                                let body = serde_json::json!({"error": "LLM analysis unavailable"}).to_string();
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

pub(crate) fn route_guard_allow_rules(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let rules = audit::list_guard_allow_rules();
                let body = serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_guard_allow_rules_remove(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct RemoveReq { id: String }
                match serde_json::from_slice::<RemoveReq>(&body_bytes) {
                    Ok(req) => match audit::remove_guard_allow_rule(&req.id) {
                        Ok(()) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string("{}").with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = format!(r#"{{"error":"{}"}}"#, e.replace('"', "\\\""));
                            let _ = request.respond(
                                tiny_http::Response::from_string(body)
                                    .with_status_code(500)
                                    .with_header(json_header),
                            );
                        }
                    },
                    Err(e) => {
                        let body = format!(r#"{{"error":"{}"}}"#, e.to_string().replace('"', "'"));
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }
