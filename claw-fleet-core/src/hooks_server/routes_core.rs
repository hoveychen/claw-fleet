//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

/// `GET /fleet_skill` — the bundled Fleet SKILL.md, verbatim.
///
/// Deliberately not a Backend method: there is nothing per-host about it. The
/// desktop already holds the same `include_str!` constant and reads it straight
/// out of the binary (`save_skill_file`), so adding a trait method would mean
/// two implementations that both return one compile-time string. What the
/// browser build lacks is not the *data path* but the *file* — it can neither
/// see the constant nor be handed a destination to write it to — so this route
/// exists to let it offer the file as a download.
///
/// Not on the public whitelist (`routes::is_public`), so a scoped token cannot
/// reach it; on `fleet webui` it rides that port's blanket `no_auth`.
pub(crate) fn route_fleet_skill(request: tiny_http::Request) {
    let header: tiny_http::Header = "Content-Type: text/markdown; charset=utf-8"
        .parse()
        .unwrap();
    let _ = request.respond(
        tiny_http::Response::from_string(crate::FLEET_SKILL_MD).with_header(header),
    );
}

pub(crate) fn route_health(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let body = format!(
                    r#"{{"version":"{}","status":"ok"}}"#,
                    env!("CARGO_PKG_VERSION")
                );
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_sessions(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;
    let search_index = ctx.search_index;

                let sessions = ctx.snapshot.sessions();
                // Incrementally update the search index with the latest session list.
                let pairs: Vec<(String, String)> = sessions
                    .iter()
                    .map(|s| (s.jsonl_path.clone(), s.id.clone()))
                    .collect();
                search_index.index_batch(&pairs);
                let body = serde_json::to_string(&sessions).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

/// `/interrupt_agent_session?path=<uri>` — stop one session through its own
/// source, for the sources with no per-session process to signal (dsh).
pub(crate) fn route_interrupt_agent_session(
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
) {
    let Some(target) = query.get("path").filter(|p| !p.is_empty()) else {
        let _ = request.respond(tiny_http::Response::empty(400));
        return;
    };
    match crate::agent_source::interrupt_session_at(target) {
        Ok(()) => {
            let _ = request.respond(
                tiny_http::Response::from_string(r#"{"ok":true}"#).with_header(json_header),
            );
        }
        Err(e) => {
            let body = serde_json::json!({ "error": e }).to_string();
            let _ = request
                .respond(tiny_http::Response::from_string(body).with_header(json_header));
        }
    }
}

pub(crate) fn route_interrupt(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let pid: u32 = query.get("pid").and_then(|s| s.parse().ok()).unwrap_or(0);
                if pid == 0 {
                    let _ = request.respond(tiny_http::Response::empty(400));
                    return;
                }
                match crate::session::interrupt_pid_impl(pid) {
                    Ok(()) => {
                        let _ = request.respond(
                            tiny_http::Response::from_string(r#"{"ok":true}"#)
                                .with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = format!(r#"{{"error":"{}"}}"#, e);
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                    }
                }
            }

pub(crate) fn route_stop(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let pid: u32 = query.get("pid").and_then(|s| s.parse().ok()).unwrap_or(0);
                if pid == 0 {
                    let _ = request.respond(tiny_http::Response::empty(400));
                    return;
                }
                let force: bool = query.get("force").map(|s| s == "true").unwrap_or(false);
                #[cfg(unix)]
                {
                    // Probe first so a stale pid still 500s; then take the whole
                    // tree, or the agent's tool children outlive it.
                    if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
                        let err = std::io::Error::last_os_error().to_string();
                        let body = format!(r#"{{"error":"{}"}}"#, err);
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                        return;
                    }
                    match crate::session::kill_pid_tree(pid, force) {
                        Ok(()) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(r#"{"ok":true}"#)
                                    .with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = format!(r#"{{"error":"{}"}}"#, e);
                            let _ = request.respond(
                                tiny_http::Response::from_string(body)
                                    .with_status_code(500)
                                    .with_header(json_header),
                            );
                        }
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = (pid, force);
                    let _ = request.respond(tiny_http::Response::empty(400));
                }
            }

pub(crate) fn route_stop_workspace(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let path_param = query
                    .get("path")
                    .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                    .unwrap_or_default();
                if path_param.is_empty() {
                    let _ = request.respond(tiny_http::Response::empty(400));
                    return;
                }
                match crate::session::kill_workspace_impl(&path_param) {
                    Ok(()) => {
                        let _ = request.respond(
                            tiny_http::Response::from_string(r#"{"ok":true}"#)
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
                }
            }

pub(crate) fn route_sources_prefix(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
                // Expected: ["sources", "<name>", "account"|"usage"]
                if parts.len() == 3 {
                    let source_name = parts[1];
                    let kind = parts[2];

                    // Check sources config before serving
                    let config = crate::agent_source::SourcesConfig::load();
                    if !config.is_source_enabled(source_name) {
                        let body = format!(r#"{{"error":"Source '{}' is disabled"}}"#, source_name);
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(403)
                                .with_header(json_header),
                        );
                        return;
                    }

                    if let Some(source) = agent_source::find_source_by_api_name(sources, source_name) {
                        let result = match kind {
                            "account" => source.fetch_account(),
                            "usage" => source.fetch_usage(),
                            _ => Err(format!("Unknown endpoint: {kind}")),
                        };
                        match result {
                            Ok(val) => {
                                let body = serde_json::to_string(&val).unwrap_or_default();
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body).with_header(json_header),
                                );
                            }
                            Err(e) => {
                                let body = format!(r#"{{"error":"{}"}}"#, e.replace('"', "\\\""));
                                let _ = request.respond(
                                    tiny_http::Response::from_string(body)
                                        .with_status_code(404)
                                        .with_header(json_header),
                                );
                            }
                        }
                    } else {
                        let body = format!(r#"{{"error":"Unknown source: {}"}}"#, source_name);
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(404)
                                .with_header(json_header),
                        );
                    }
                } else {
                    let _ = request.respond(tiny_http::Response::empty(404));
                }
            }

pub(crate) fn route_setup_status(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let sessions = ctx.snapshot.sessions();
                let detected_tools = crate::detect_installed_tools(&sessions);
                let (cli_installed, cli_path) = crate::check_cli_installed();
                let claude_dir_exists = get_claude_dir().map_or(false, |d| d.is_dir());
                let logged_in = crate::account::read_keychain_credentials().is_ok();
                let has_sessions = !sessions.is_empty();

                let status = crate::backend::SetupStatus {
                    cli_installed,
                    cli_path,
                    claude_dir_exists,
                    detected_tools,
                    logged_in,
                    has_sessions,
                    credentials_valid: None,
                };
                let body = serde_json::to_string(&status).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_usage_summaries(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let summaries = crate::agent_source::fetch_usage_summaries_from_sources(sources);
                let body = serde_json::to_string(&summaries).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_today_usage(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let sessions = ctx.snapshot.sessions();
                let usage = crate::today_usage::today_usage(&sessions);
                let body = serde_json::to_string(&usage).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_cloud_usage(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    _query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    _path: &str,
) {
    // One customer per container, so this container's usage IS the customer's.
    // Reuses the session snapshot; no extra JSONL scan beyond it.
    let sessions = ctx.snapshot.sessions();
    let usage = crate::today_usage::cloud_usage(&sessions);
    let body = serde_json::to_string(&usage).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_today_usage_breakdown(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    _query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    _path: &str,
) {
    let sources = ctx.sources;
    let sessions = ctx.snapshot.sessions();
    let breakdown = crate::today_usage::today_usage_breakdown(&sessions);
    let body = serde_json::to_string(&breakdown).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_usage_range_breakdown(
    ctx: &ServeCtx,
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
    let sessions = ctx.snapshot.sessions();
    let breakdown = crate::today_usage::usage_range_breakdown(&sessions, from_ms, to_ms);
    let body = serde_json::to_string(&breakdown).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_session_decisions(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let raw_id = query.get("session_id").map(|s| s.as_str()).unwrap_or("");
                let session_id = percent_decode_str(raw_id).decode_utf8_lossy().to_string();
                let jsonl_path = query.get("jsonl_path").map(|s| {
                    percent_decode_str(s.as_str()).decode_utf8_lossy().to_string()
                });
                let resolved = if jsonl_path.is_none() {
                    ctx.snapshot.sessions()
                        .into_iter()
                        .find(|s| s.id == session_id)
                        .map(|s| s.jsonl_path)
                } else {
                    None
                };
                let path = jsonl_path
                    .as_deref()
                    .or(resolved.as_deref())
                    .map(std::path::Path::new);
                let records = crate::decision_history::list_session_records_with_jsonl(
                    &session_id,
                    path,
                );
                let body = serde_json::to_string(&records).unwrap_or_else(|_| "[]".to_string());
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_messages(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let tail: Option<usize> = query.get("tail").and_then(|s| s.parse().ok());
                if let Some(source) = find_source_for_path(sources, &file_path) {
                    let result = match tail {
                        Some(n) => source.get_messages_tail(&file_path, n),
                        None => source.get_messages(&file_path),
                    };
                    match result {
                        Ok(mut messages) => {
                            // Trim oversized tool output before it crosses the
                            // HTTP boundary, mirroring LocalBackend. Only the
                            // tail path (SessionDetail) is trimmed; a full
                            // `get_messages` fetch keeps everything intact.
                            if tail.is_some() {
                                crate::message_trim::trim_messages_for_transport(&mut messages);
                            }
                            let body = serde_json::to_string(&messages).unwrap_or_default();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body).with_header(json_header),
                            );
                        }
                        Err(_) => {
                            let _ = request.respond(tiny_http::Response::empty(404));
                        }
                    }
                } else {
                    let _ = request.respond(tiny_http::Response::empty(404));
                }
            }

/// Full, untrimmed tool output for one `tool_use_id`. The remote counterpart of
/// `Backend::get_tool_result_full`, called when the reader expands a card whose
/// tail payload was trimmed by [`crate::message_trim`].
pub(crate) fn route_tool_result(
    _ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    _path: &str,
) {
    let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
    let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
    let tool_use_id = query.get("tool_use_id").map(|s| s.as_str()).unwrap_or("");
    match crate::message_trim::extract_full_tool_result(
        std::path::Path::new(&file_path),
        tool_use_id,
    ) {
        Ok(value) => {
            let body = serde_json::to_string(&value).unwrap_or_default();
            let _ = request.respond(
                tiny_http::Response::from_string(body).with_header(json_header),
            );
        }
        Err(_) => {
            let _ = request.respond(tiny_http::Response::empty(404));
        }
    }
}

pub(crate) fn route_file_size(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let uri = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let resolved = find_source_for_path(sources, &uri)
                    .and_then(|s| s.resolve_file_path(&uri));
                let size = resolved
                    .and_then(|p| std::fs::metadata(&p).ok())
                    .map(|m| m.len())
                    .unwrap_or(0);
                let body = format!(r#"{{"size":{}}}"#, size);
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_tail(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let sources = ctx.sources;

                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let uri = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let offset: u64 = query
                    .get("offset")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                let Some(source) = find_source_for_path(sources, &uri) else {
                    let _ = request.respond(tiny_http::Response::empty(404));
                    return;
                };

                // Source-aware incremental follow (the RemoteBackend live-tail
                // surface, mirroring the mobile relay `serve_tail_delta` and the
                // desktop `emit_tail_lines`): Claude gets the byte-offset raw
                // tail; Codex re-normalizes its folded rollout so the emitted
                // `session-tail` rows are renderable messages the desktop dedups
                // by their stable `uuid`. A raw byte slice of a Codex rollout is
                // not a message — the old code here emitted un-renderable
                // `response_item` records (the "reply never shows" bug).
                match source.tail_incremental(&uri, offset) {
                    Ok((mut lines, new_offset)) => {
                        // Collapse oversized tool output (e.g. a Claude `Read` of
                        // an image → huge base64) to a marked preview before it
                        // crosses the wire, matching `/messages?tail=N` and the
                        // desktop watcher; the full payload is recovered via
                        // `get_tool_result_full` only when the row is expanded.
                        crate::message_trim::trim_messages_for_transport(&mut lines);
                        let body = serde_json::json!({
                            "lines": lines,
                            "newOffset": new_offset
                        })
                        .to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

pub(crate) fn route_live_thinking(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let raw = query.get("session_id").map(|s| s.as_str()).unwrap_or("");
                let session_id = percent_decode_str(raw).decode_utf8_lossy().to_string();
                // Sidecars live on the machine running `fleet serve`, so read
                // them directly rather than per-source. Serializes to `null`
                // when there's no live sidecar for this session.
                let lt = crate::live_thinking::read_live_thinking(&session_id);
                let body = serde_json::to_string(&lt).unwrap_or_else(|_| "null".into());
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_decision_asset(
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
                let (id, qidx, rel) = (dec("id"), dec("qidx"), dec("path"));
                match crate::mcp_ipc::read_decision_asset(&id, &qidx, &rel) {
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

/// `/decision_asset/<id>/<qidx>/<relpath…>` — the same bytes
/// [`route_decision_asset`] answers, addressed as a path so the served
/// `index.html` can reach the question's images through relative refs. The
/// browser build's card preview points here because `fleet-decision://` only
/// exists on the Tauri webview.
///
/// Split mirrors the desktop protocol handler (`gui/mod.rs`): `splitn(3, '/')`
/// over the tail, decoding each part, so `rel` keeps its own slashes. Traversal
/// is `read_decision_asset`'s job — it applies the same canonicalize-and-prefix
/// defense as `wiki::get_file_in` — so this arm deliberately adds no second
/// opinion about what is safe.
pub(crate) fn route_decision_asset_prefix(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let dec = |s: &str| percent_decode_str(s).decode_utf8_lossy().to_string();
    let tail = &path[crate::routes::DECISION_ASSET_PREFIX.len()..];
    let mut segs = tail.splitn(3, '/');
    let id = dec(segs.next().unwrap_or(""));
    let qidx = dec(segs.next().unwrap_or(""));
    let rel = dec(segs.next().unwrap_or(""));
    match crate::mcp_ipc::read_decision_asset(&id, &qidx, &rel) {
        Ok(f) => {
            let mime_header: tiny_http::Header =
                format!("Content-Type: {}", f.mime).parse().unwrap();
            let _ = request
                .respond(tiny_http::Response::from_data(f.bytes).with_header(mime_header));
        }
        Err(_) => {
            let _ = request.respond(tiny_http::Response::empty(404));
        }
    }
}

pub(crate) fn route_review_doc(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                let resolved = serde_json::from_slice::<crate::mcp_ipc::ReviewDoc>(&body_bytes)
                    .map_err(|e| format!("bad /review_doc body: {e}"))
                    .and_then(|doc| crate::mcp_ipc::read_review_doc(&doc));
                match resolved {
                    Ok(content) => {
                        let body = serde_json::to_string(&content).unwrap_or_default();
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

pub(crate) fn route_task_plans(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let workspace_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                // Optional `session` scopes the result to that session's focused
                // plan; absent → every plan (matches the direct fn contract).
                let session_id = query.get("session").map(|s| s.as_str());
                let plans = crate::prd_tasks::list_workspace_task_plans(
                    std::path::Path::new(&workspace_path),
                    session_id,
                );
                let body = serde_json::to_string(&plans).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

/// `GET /plan_forest?path=<workspace>` — the workspace's plan forest with its
/// handoff chains joined in. Chains are filtered to the requested workspace here
/// (they are stored per-machine), matching what `LocalBackend` does, so both
/// transports return the same shape.
pub(crate) fn route_plan_forest(
    _ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    _path: &str,
) {
    let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
    let workspace_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
    let chains = crate::handoff::list_chains()
        .into_iter()
        .filter(|c| c.workspace_path == workspace_path)
        .collect();
    let forest = crate::plan_forest::build(std::path::Path::new(&workspace_path), chains);
    let body = serde_json::to_string(&forest).unwrap_or_default();
    let _ = request.respond(tiny_http::Response::from_string(body).with_header(json_header));
}

pub(crate) fn route_search(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {
    let search_index = ctx.search_index;

                let q = query.get("q").cloned().unwrap_or_default();
                let limit: usize = query
                    .get("limit")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(50);
                let hits = search_index.search(&q, limit).unwrap_or_default();
                let body = serde_json::to_string(&hits).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }
