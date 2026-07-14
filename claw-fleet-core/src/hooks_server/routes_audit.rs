//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_audit(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;
    let audit_history = ctx.audit_history.clone();

                use crate::audit::extract_audit_events;
                let sessions = scan_all_sources(sources);
                let active_ids: std::collections::HashSet<String> = sessions
                    .iter()
                    .filter(|s| !matches!(s.status, SessionStatus::Idle))
                    .map(|s| s.id.clone())
                    .collect();
                let active: Vec<&SessionInfo> = sessions
                    .iter()
                    .filter(|s| active_ids.contains(&s.id))
                    .collect();
                let total = active.len();

                // Scan active sessions for audit events.
                let mut live_events = Vec::new();
                for session in &active {
                    let path = &session.jsonl_path;
                    if let Some(src) = find_source_for_path(sources, path) {
                        if let Ok(messages) = src.get_messages(path) {
                            let events = extract_audit_events(&messages, session);
                            live_events.extend(events);
                        }
                    }
                }

                // Persist events from idle sessions into history.
                let idle: Vec<&SessionInfo> = sessions
                    .iter()
                    .filter(|s| matches!(s.status, SessionStatus::Idle))
                    .collect();
                let mut idle_events = Vec::new();
                for session in &idle {
                    let path = &session.jsonl_path;
                    if let Some(src) = find_source_for_path(sources, path) {
                        if let Ok(messages) = src.get_messages(path) {
                            let events = extract_audit_events(&messages, session);
                            idle_events.extend(events);
                        }
                    }
                }

                let mut hist = audit_history.lock().unwrap();
                hist.persist_evicted(idle_events);
                hist.remove_sessions(&active_ids);
                let mut all_events: Vec<_> = hist.events().to_vec();
                drop(hist);
                all_events.extend(live_events);

                all_events.sort_by(|a, b| b.timestamp.cmp(&a.timestamp));
                let summary = crate::audit::AuditSummary {
                    events: all_events,
                    total_sessions_scanned: total,
                };
                let body = serde_json::to_string(&summary).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_audit_pattern_info(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let (version, path) = crate::pattern_update::get_patterns_info();
                let body = serde_json::json!({
                    "version": version,
                    "path": path,
                }).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_audit_check_update(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let msg = crate::pattern_update::check_update_now();
                let body = serde_json::json!({ "message": msg }).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_audit_rules(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let rules = crate::audit::get_all_rules();
                let body = serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_audit_rules_toggle(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct ToggleReq { id: String, enabled: bool }
                match serde_json::from_slice::<ToggleReq>(&body_bytes) {
                    Ok(req) => match crate::audit::set_rule_enabled(&req.id, req.enabled) {
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

pub(crate) fn route_audit_rules_save(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::audit::AuditRuleInfo>(&body_bytes) {
                    Ok(rule) => match crate::audit::save_custom_rule(rule) {
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

pub(crate) fn route_audit_rules_delete(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct DeleteReq { id: String }
                match serde_json::from_slice::<DeleteReq>(&body_bytes) {
                    Ok(req) => match crate::audit::delete_custom_rule(&req.id) {
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

pub(crate) fn route_audit_rules_suggest(
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
                struct SuggestReq { concern: String, lang: String }
                match serde_json::from_slice::<SuggestReq>(&body_bytes) {
                    Ok(req) => {
                        let existing_tags: Vec<String> = crate::audit::get_all_rules()
                            .iter()
                            .map(|r| r.tag.clone())
                            .collect();
                        let prompt = crate::audit::build_suggest_rules_prompt(
                            &req.concern, &req.lang, &existing_tags,
                        );
                        let llm_cfg = llm_config.lock().unwrap().clone();
                        let provider = crate::llm_provider::resolve_provider(&llm_cfg.provider);
                        match provider {
                            Some(p) => {
                                match crate::llm_usage::complete_accounted(
                                    p.as_ref(),
                                    &prompt,
                                    &llm_cfg.standard_model,
                                    std::time::Duration::from_secs(120),
                                    crate::llm_usage::SCENARIO_AUDIT_RULES,
                                ) {
                                    Some(resp) => {
                                        let json_str = resp.trim();
                                        let json_str = json_str
                                            .strip_prefix("```json")
                                            .or_else(|| json_str.strip_prefix("```"))
                                            .unwrap_or(json_str);
                                        let json_str = json_str.strip_suffix("```").unwrap_or(json_str).trim();
                                        match serde_json::from_str::<Vec<crate::audit::SuggestedRule>>(json_str) {
                                            Ok(suggestions) => {
                                                let body = serde_json::to_string(&suggestions).unwrap_or_else(|_| "[]".into());
                                                let _ = request.respond(
                                                    tiny_http::Response::from_string(body).with_header(json_header),
                                                );
                                            }
                                            Err(e) => {
                                                let body = format!(r#"{{"error":"Failed to parse LLM response: {}"}}"#, e.to_string().replace('"', "'"));
                                                let _ = request.respond(
                                                    tiny_http::Response::from_string(body)
                                                        .with_status_code(500)
                                                        .with_header(json_header),
                                                );
                                            }
                                        }
                                    }
                                    None => {
                                        let body = r#"{"error":"LLM did not return a response"}"#;
                                        let _ = request.respond(
                                            tiny_http::Response::from_string(body)
                                                .with_status_code(500)
                                                .with_header(json_header),
                                        );
                                    }
                                }
                            }
                            None => {
                                let body = r#"{"error":"No LLM provider available"}"#;
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_status_code(500)
                                        .with_header(json_header),
                                );
                            }
                        }
                    }
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
