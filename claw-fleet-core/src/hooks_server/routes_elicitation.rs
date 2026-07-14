//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_apply_elicitation_hook(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                match hooks::apply_elicitation_hook() {
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

pub(crate) fn route_remove_elicitation_hook(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                match hooks::remove_elicitation_hook() {
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

pub(crate) fn route_elicitation_pending(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let ids = elicitation::list_pending_requests();
                let sessions = scan_all_sources(sources);
                let mut requests = Vec::new();
                for id in &ids {
                    if let Some(mut req) = elicitation::read_request(id) {
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
                // Cards whose wait timed out live in the parked store instead of the
                // channel's request dir — the producer that was blocking on them is
                // gone. They stay pending here until the user resolves them.
                for mut req in crate::parked::list_requests::<elicitation::ElicitationRequest>(crate::parked::ParkedKind::Elicitation) {
                    if let Some(sess) = sessions.iter().find(|s| s.id == req.session_id) {
                        if req.workspace_name.is_empty() {
                            req.workspace_name = sess.workspace_name.clone();
                        }
                        if req.ai_title.is_none() {
                            req.ai_title = sess.ai_title.clone();
                        }
                    }
                    requests.push(req);
                }
                let body = serde_json::to_string(&requests).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_elicitation_respond(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<elicitation::ElicitationResponse>(&body_bytes) {
                    Ok(resp) => {
                        // A parked card has no producer left polling for a response
                        // file, so `deliver` resumes the session with the answer
                        // instead (or drops the card when the user dismissed it).
                        let outcome =
                            crate::parked::deliver(&resp.id, &resp, resp.declined, elicitation::write_response);
                        match outcome {
                            Ok(()) => {
                                // Don't cleanup here — the `fleet elicitation` CLI
                                // polls for the response and does cleanup itself.
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

pub(crate) fn route_elicitation_upload(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let raw_name = query.get("name").map(|s| s.as_str()).unwrap_or("");
                let decoded = percent_decode_str(raw_name).decode_utf8_lossy().to_string();
                let safe_name: String = std::path::Path::new(&decoded)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "attachment.bin".to_string());

                const MAX: u64 = crate::backend::MAX_ATTACHMENT_BYTES;

                // Reject early via Content-Length if the client declared one.
                if let Some(len) = request.body_length() {
                    if (len as u64) > MAX {
                        let body = serde_json::json!({
                            "error": format!("attachment too large: {len} bytes (max {MAX})")
                        })
                        .to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(413)
                                .with_header(json_header),
                        );
                        return;
                    }
                }

                // Read at most MAX+1 bytes so we can still detect oversized
                // streams that lied about (or omitted) Content-Length.
                let mut body_bytes = Vec::new();
                let mut limited = std::io::Read::take(request.as_reader(), MAX + 1);
                let _ = std::io::Read::read_to_end(&mut limited, &mut body_bytes);
                if (body_bytes.len() as u64) > MAX {
                    let body = serde_json::json!({
                        "error": format!("attachment too large: >{MAX} bytes")
                    })
                    .to_string();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_status_code(413)
                            .with_header(json_header),
                    );
                    return;
                }

                // Pasted bytes go into the persistent user-attachment store: the
                // path we return here is spliced into the prompt / decision
                // answer, so it has to survive the temp reaper for history to
                // resolve it later. Picked files still land in $TMPDIR — the
                // desktop only uploads them because the agent host can't see the
                // desktop's disk, and nothing renders them back.
                let from_clipboard = query.get("from_clipboard").is_some_and(|v| v == "1");

                let dest = if from_clipboard {
                    match crate::user_attachments::ingest_bytes(&body_bytes, &safe_name) {
                        Ok(p) => p,
                        Err(e) => {
                            let body = serde_json::json!({"error": e}).to_string();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body)
                                    .with_status_code(500)
                                    .with_header(json_header),
                            );
                            return;
                        }
                    }
                } else {
                    let dir = std::env::temp_dir().join("fleet-attachments");
                    if let Err(e) = std::fs::create_dir_all(&dir) {
                        let body = serde_json::json!({"error": format!("mkdir: {e}")}).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                        return;
                    }

                    let nanos = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos();
                    let pid = std::process::id();
                    let dest = dir.join(format!("{nanos}-{pid}-{safe_name}"));

                    if let Err(e) = std::fs::write(&dest, &body_bytes) {
                        let body = serde_json::json!({"error": format!("write: {e}")}).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                        return;
                    }
                    dest
                };

                let abs = dest.to_string_lossy().into_owned();
                let body = serde_json::json!({"path": abs}).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_user_attachment(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let dec = |key: &str| {
                    query
                        .get(key)
                        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                        .unwrap_or_default()
                };
                let (key, name) = (dec("key"), dec("name"));
                match crate::user_attachments::read_user_attachment(&key, &name) {
                    Ok(f) => {
                        let mime_header: tiny_http::Header =
                            format!("Content-Type: {}", f.mime).parse().unwrap();
                        let _ = request.respond(
                            tiny_http::Response::from_data(f.bytes).with_header(mime_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }
