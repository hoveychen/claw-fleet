//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_daily_report(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let report_store = ctx.report_store.clone();

                let date = query.get("date").cloned().unwrap_or_default();
                let store = report_store.lock().unwrap();
                match store.get_report(&date) {
                    Ok(report) => {
                        let body = serde_json::to_string(&report).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = format!("{{\"error\":\"{}\"}}", e);
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                    }
                }
            }

pub(crate) fn route_daily_report_stats(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let report_store = ctx.report_store.clone();

                let from = query.get("from").cloned().unwrap_or_default();
                let to = query.get("to").cloned().unwrap_or_default();
                let store = report_store.lock().unwrap();
                let stats = store.list_stats(&from, &to).unwrap_or_default();
                let body = serde_json::to_string(&stats).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_daily_report_generate(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let report_store = ctx.report_store.clone();

                let date = query.get("date").cloned().unwrap_or_default();
                let sessions = scan_sessions_for_date(&date);
                if sessions.is_empty() {
                    let body = r#"{"error":"no sessions found for date"}"#;
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_status_code(404)
                            .with_header(json_header),
                    );
                } else {
                    let session_refs: Vec<&SessionInfo> = sessions.iter().collect();
                    let tz = chrono::Local::now().format("%Z").to_string();
                    let report = generate_report_from_sessions(&date, &tz, &session_refs);
                    report_store.lock().unwrap().save_report(&report).ok();
                    let body = serde_json::to_string(&report).unwrap_or_default();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body).with_header(json_header),
                    );
                }
            }

pub(crate) fn route_daily_report_ai_summary(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let report_store = ctx.report_store.clone();
    let llm_config = ctx.llm_config.clone();

                let date = query.get("date").cloned().unwrap_or_default();
                let lang = query.get("lang").map(|s| s.as_str()).unwrap_or("en");
                let store = report_store.lock().unwrap();
                match store.get_report(&date) {
                    Ok(Some(report)) => {
                        drop(store);
                        let cfg = llm_config.lock().unwrap().clone();
                        let result = generate_ai_summary_routed(&cfg, &report, lang);
                        match result {
                            Some(summary) => {
                                report_store
                                    .lock()
                                    .unwrap()
                                    .update_ai_summary(&date, &summary)
                                    .ok();
                                let body = serde_json::to_string(&summary).unwrap_or_default();
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_header(json_header),
                                );
                            }
                            None => {
                                let body = r#"{"error":"AI summary generation failed"}"#;
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_status_code(500)
                                        .with_header(json_header),
                                );
                            }
                        }
                    }
                    _ => {
                        let body = r#"{"error":"report not found"}"#;
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(404)
                                .with_header(json_header),
                        );
                    }
                }
            }

pub(crate) fn route_daily_report_lessons(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let report_store = ctx.report_store.clone();
    let llm_config = ctx.llm_config.clone();

                let date = query.get("date").cloned().unwrap_or_default();
                let lang = query.get("lang").map(|s| s.as_str()).unwrap_or("en");
                let store = report_store.lock().unwrap();
                match store.get_report(&date) {
                    Ok(Some(report)) => {
                        drop(store);
                        let cfg = llm_config.lock().unwrap().clone();
                        let result = generate_lessons_routed(&cfg, &report, lang);
                        match result {
                            Some(lessons) => {
                                report_store
                                    .lock()
                                    .unwrap()
                                    .update_lessons(&date, &lessons)
                                    .ok();
                                let body = serde_json::to_string(&lessons).unwrap_or_default();
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_header(json_header),
                                );
                            }
                            None => {
                                let body = r#"{"error":"Lessons generation failed"}"#;
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_status_code(500)
                                        .with_header(json_header),
                                );
                            }
                        }
                    }
                    _ => {
                        let body = r#"{"error":"report not found"}"#;
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(404)
                                .with_header(json_header),
                        );
                    }
                }
            }

pub(crate) fn route_daily_report_append_lesson(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<Lesson>(&body_bytes) {
                    Ok(lesson) => match append_lesson_to_claude_md(&lesson) {
                        Ok(()) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string("{}")
                                    .with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = format!(r#"{{"error":"{}"}}"#, e.replace('"', "'"));
                            let _ = request.respond(
                                tiny_http::Response::from_string(body)
                                    .with_status_code(500)
                                    .with_header(json_header),
                            );
                        }
                    },
                    Err(e) => {
                        let body = format!(r#"{{"error":"invalid lesson: {}"}}"#, e.to_string().replace('"', "'"));
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }

pub(crate) fn route_managed_lessons(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let lessons = crate::lessons_store::list_lessons();
    let body = serde_json::to_string(&lessons).unwrap_or_else(|_| "[]".to_string());
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_managed_lesson_remove(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    let id = serde_json::from_slice::<serde_json::Value>(&body_bytes)
        .ok()
        .and_then(|v| v.get("id").and_then(|s| s.as_str()).map(|s| s.to_string()));
    match id {
        Some(id) => match crate::lessons_store::remove_lesson(&id) {
            Ok(()) => {
                let _ = request.respond(
                    tiny_http::Response::from_string("{}").with_header(json_header),
                );
            }
            Err(e) => {
                let body = format!(r#"{{"error":"{}"}}"#, e.replace('"', "'"));
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(500)
                        .with_header(json_header),
                );
            }
        },
        None => {
            let _ = request.respond(
                tiny_http::Response::from_string(r#"{"error":"missing id"}"#)
                    .with_status_code(400)
                    .with_header(json_header),
            );
        }
    }
}
