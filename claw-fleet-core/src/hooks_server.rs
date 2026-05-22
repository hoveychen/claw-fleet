//! Shared lib backing the `fleet serve` and `fleet-hooks-server` binaries.
//!
//! Phase 4 P1 extracted the 3000-line `cmd_serve` body that used to live in
//! `fleet-cli/src/main.rs` into this module so both binaries can call the
//! same `serve()` function instead of trampolining through `execvp`.

pub mod sse;

pub use sse::{handle_sse_upgrade, SseBroadcaster, SseClient};

pub fn serve(port: u16, token: String, port_file: Option<std::path::PathBuf>) {
    use std::io::{Read, Seek, SeekFrom};
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    use percent_encoding::percent_decode_str;

    use crate::agent_source::{self, build_sources, find_source_for_path};
    use crate::audit;
    use crate::claude_analyze;
    use crate::daily_report::{
        append_lesson_to_claude_md, generate_ai_summary, generate_lessons,
        generate_report_from_sessions, scan_sessions_for_date, Lesson, ReportStore,
    };
    use crate::elicitation;
    use crate::guard;
    use crate::hooks;
    use crate::interaction_mode;
    use crate::llm_provider::{self, LlmConfig};
    use crate::memory;
    use crate::plan_approval;
    use crate::plugins;
    use crate::prd_discipline;
    use crate::search_index::SearchIndex;
    use crate::session::{get_claude_dir, scan_all_sources, SessionInfo, SessionStatus};
    use crate::skills;

    let sources = build_sources();

    // Open the daily report store.
    let report_store = Arc::new(Mutex::new(
        ReportStore::open().expect("report store open"),
    ));

    // LLM provider config — defaults to Claude, updated via /llm/config endpoint.
    let llm_config: Arc<Mutex<LlmConfig>> = Arc::new(Mutex::new(LlmConfig::default()));

    // Load the persistent audit history.
    let audit_history = Arc::new(Mutex::new(
        crate::audit::AuditHistory::load(),
    ));

    // Open the full-text search index (stored on the remote host).
    let search_index = {
        let db_path = crate::session::real_home_dir()
            .expect("cannot determine home dir")
            .join(".fleet")
            .join("fleet-search.db");
        SearchIndex::open_at(&db_path).unwrap_or_else(|e| {
            eprintln!("[fleet serve] search index open failed, retrying fresh: {e}");
            let _ = std::fs::remove_file(&db_path);
            SearchIndex::open_at(&db_path).expect("search index open failed twice")
        })
    };

    let sse = SseBroadcaster::new();

    let addr = format!("127.0.0.1:{}", port);
    let server = tiny_http::Server::http(&addr).unwrap_or_else(|e| {
        eprintln!("Error: cannot bind to {}: {}", addr, e);
        std::process::exit(1);
    });
    let actual_port = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(port);
    if let Some(pf) = port_file.as_ref() {
        if let Err(e) = std::fs::write(pf, actual_port.to_string()) {
            eprintln!("[fleet serve] failed to write port-file {}: {}", pf.display(), e);
        }
    }
    // Always write the canonical port + token files under ~/.claude/fleet/ so
    // `fleet session status` (called from inside a managed Claude session) can
    // discover the supervisor regardless of how fleet serve was started.
    if let (Some(pp), Some(tp)) = (
        crate::launchd::port_file_path(),
        crate::launchd::token_file_path(),
    ) {
        if let Some(dir) = pp.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let _ = std::fs::write(&pp, actual_port.to_string());
        let _ = std::fs::write(&tp, &token);
    }
    {
        use std::io::Write as _;
        println!("FLEET_PROBE_PORT={}", actual_port);
        let _ = std::io::stdout().flush();
    }
    eprintln!("[fleet serve] listening on 127.0.0.1:{} (version {})", actual_port, env!("CARGO_PKG_VERSION"));

    // ── Startup zombie recovery ────────────────────────────────────────────
    // If `fleet serve` was killed mid-flight (host crash, log-out, etc.) any
    // sessions whose pids are gone are still recorded as Running/Pending in
    // fleet-sessions.json. Sweep them to Complete before the tick loop starts
    // so the kanban board reflects reality on first paint.
    match crate::supervisor::migrate_zombie_running() {
        Ok(0) => {}
        Ok(n) => eprintln!("[fleet serve] recovered {n} zombie session(s) on startup"),
        Err(e) => eprintln!("[fleet serve] zombie recovery failed: {e}"),
    }
    match crate::supervisor::gc_orphan_worktrees() {
        Ok(0) => {}
        Ok(n) => eprintln!("[fleet serve] reaped {n} orphan worktree(s) on startup"),
        Err(e) => eprintln!("[fleet serve] worktree gc failed: {e}"),
    }

    // ── Supervisor tick thread (REMOVED in Phase 3) ─────────────────────────
    // The legacy `supervisor::tick` loop spawned queued fleet sessions and
    // reaped exited pids out of `fleet-sessions.json`. Phase 3 retires that
    // path: the `fleet-task` standalone binary owns task lifecycle now, and
    // `fleet serve` is reduced to hook endpoints (guard / elicitation /
    // plan-approval / accounts / llm / feishu / daily_report / search /
    // audit). Killing fleet serve no longer interrupts running tasks.

    // ── Background SSE broadcaster thread ──────────────────────────────────
    // Polls for session changes, waiting alerts, guard/elicitation requests
    // and pushes them to connected SSE clients.
    {
        let sse_bg = sse.clone();
        std::thread::spawn(move || {
            let sources_bg = build_sources();
            let mut prev_sessions_json = String::new();
            let mut prev_alert_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut prev_guard_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut prev_elicit_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut prev_plan_approval_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));

                if sse_bg.client_count() == 0 {
                    continue;
                }

                // A SSE client is connected — mark this serve as a live consumer
                // so `fleet guard`/`fleet elicitation` know the head is reachable.
                crate::consumer_heartbeat::write_heartbeat();

                // Broadcast session updates
                let sessions = scan_all_sources(&sources_bg);
                let json = serde_json::to_string(&sessions).unwrap_or_default();
                if json != prev_sessions_json {
                    sse_bg.broadcast("sessions-updated", &json);
                    prev_sessions_json = json;
                }

                // Broadcast waiting alerts (simple detection from session status)
                let waiting_ids: std::collections::HashSet<String> = sessions
                    .iter()
                    .filter(|s| {
                        !s.is_subagent
                            && s.status == crate::session::SessionStatus::WaitingInput
                    })
                    .map(|s| s.id.clone())
                    .collect();
                for sess in &sessions {
                    if waiting_ids.contains(&sess.id) && !prev_alert_ids.contains(&sess.id) {
                        let alert = crate::backend::WaitingAlert {
                            session_id: sess.id.clone(),
                            workspace_name: sess.workspace_name.clone(),
                            summary: sess.last_message_preview.clone().unwrap_or_default(),
                            detected_at_ms: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_millis() as u64,
                            jsonl_path: sess.jsonl_path.clone(),
                            source: sess.agent_source.clone(),
                        };
                        if let Ok(json) = serde_json::to_string(&alert) {
                            sse_bg.broadcast("waiting-alert", &json);
                        }
                    }
                }
                prev_alert_ids = waiting_ids;

                // Broadcast new guard requests
                let guard_ids: std::collections::HashSet<String> =
                    guard::list_pending_requests().into_iter().collect();
                for id in &guard_ids {
                    if prev_guard_ids.insert(id.clone()) {
                        if let Some(mut req) = guard::read_request(id) {
                            if let Some(s) = sessions.iter().find(|s| s.id == req.session_id) {
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = s.workspace_name.clone();
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = s.ai_title.clone();
                                }
                            }
                            if let Ok(json) = serde_json::to_string(&req) {
                                sse_bg.broadcast("guard-request", &json);
                            }
                        }
                    }
                }
                // Broadcast dismissed guard requests (answered / cleaned up)
                for id in prev_guard_ids.difference(&guard_ids) {
                    if let Ok(json) = serde_json::to_string(id) {
                        sse_bg.broadcast("guard-dismissed", &json);
                    }
                }
                prev_guard_ids.retain(|id| guard_ids.contains(id));

                // Broadcast new elicitation requests
                let elicit_ids: std::collections::HashSet<String> =
                    elicitation::list_pending_requests().into_iter().collect();
                for id in &elicit_ids {
                    if prev_elicit_ids.insert(id.clone()) {
                        if let Some(mut req) = elicitation::read_request(id) {
                            if let Some(s) = sessions.iter().find(|s| s.id == req.session_id) {
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = s.workspace_name.clone();
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = s.ai_title.clone();
                                }
                            }
                            if let Ok(json) = serde_json::to_string(&req) {
                                sse_bg.broadcast("elicitation-request", &json);
                            }
                        }
                    }
                }
                // Broadcast dismissed elicitation requests (answered / cleaned up)
                for id in prev_elicit_ids.difference(&elicit_ids) {
                    if let Ok(json) = serde_json::to_string(id) {
                        sse_bg.broadcast("elicitation-dismissed", &json);
                    }
                }
                prev_elicit_ids.retain(|id| elicit_ids.contains(id));

                // Broadcast new plan-approval requests
                let plan_approval_ids: std::collections::HashSet<String> =
                    plan_approval::list_pending_requests().into_iter().collect();
                for id in &plan_approval_ids {
                    if prev_plan_approval_ids.insert(id.clone()) {
                        if let Some(mut req) = plan_approval::read_request(id) {
                            if let Some(s) = sessions.iter().find(|s| s.id == req.session_id) {
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = s.workspace_name.clone();
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = s.ai_title.clone();
                                }
                            }
                            if let Ok(json) = serde_json::to_string(&req) {
                                sse_bg.broadcast("plan-approval-request", &json);
                            }
                        }
                    }
                }
                // Broadcast dismissed plan-approval requests (answered / cleaned up)
                for id in prev_plan_approval_ids.difference(&plan_approval_ids) {
                    if let Ok(json) = serde_json::to_string(id) {
                        sse_bg.broadcast("plan-approval-dismissed", &json);
                    }
                }
                prev_plan_approval_ids.retain(|id| plan_approval_ids.contains(id));
            }
        });
    }

    let expected_auth = format!("Bearer {}", token);

    for mut request in server.incoming_requests() {
        let url = request.url().to_string();
        let (path, query_str) = match url.split_once('?') {
            Some((p, q)) => (p, q),
            None => (url.as_str(), ""),
        };

        let query = parse_query(query_str);

        // Auth check — support both header and query param (for SSE EventSource)
        let auth_ok = request
            .headers()
            .iter()
            .any(|h| h.field.equiv("authorization")
                && h.value.as_str() == expected_auth.as_str());

        if !auth_ok {
            if query.get("token").map(|t| t.as_str()) != Some(&token) {
                let _ = request.respond(tiny_http::Response::empty(401));
                continue;
            }
        }

        // Handle SSE endpoint — takes over the connection.
        if path == "/events" {
            handle_sse_upgrade(request, &sse);
            continue;
        }

        let json_header: tiny_http::Header = "Content-Type: application/json".parse().unwrap();

        match path {
            "/health" => {
                let body = format!(
                    r#"{{"version":"{}","status":"ok"}}"#,
                    env!("CARGO_PKG_VERSION")
                );
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/sessions" => {
                let sessions = scan_all_sources(&sources);
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

            "/stop" => {
                let pid: u32 = query.get("pid").and_then(|s| s.parse().ok()).unwrap_or(0);
                if pid == 0 {
                    let _ = request.respond(tiny_http::Response::empty(400));
                    continue;
                }
                let force: bool = query.get("force").map(|s| s == "true").unwrap_or(false);
                #[cfg(unix)]
                {
                    let signal = if force { libc::SIGKILL } else { libc::SIGTERM };
                    let ret = unsafe { libc::kill(pid as libc::pid_t, signal) };
                    if ret == 0 {
                        let _ = request.respond(
                            tiny_http::Response::from_string(r#"{"ok":true}"#)
                                .with_header(json_header),
                        );
                    } else {
                        let err = std::io::Error::last_os_error().to_string();
                        let body = format!(r#"{{"error":"{}"}}"#, err);
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(500)
                                .with_header(json_header),
                        );
                    }
                }
                #[cfg(not(unix))]
                {
                    let _ = (pid, force);
                    let _ = request.respond(tiny_http::Response::empty(400));
                }
            }

            // Phase 4 P3: `/stop_workspace`, `/resume_session`, `/auto_resume_config`
            // removed. Task lifecycle is owned by fleet-task; auto-resume of
            // individual Claude Code sessions is a per-task concern now.
            // Remote callers hitting these paths receive the catch-all 404
            // below.

            // ── Unified /sources/{name}/account and /sources/{name}/usage ──
            _ if path.starts_with("/sources/") => {
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
                        continue;
                    }

                    if let Some(source) = agent_source::find_source_by_api_name(&sources, source_name) {
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

            "/setup-status" => {
                let sessions = scan_all_sources(&sources);
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

            "/usage_summaries" => {
                let summaries = crate::agent_source::fetch_usage_summaries_from_sources(&sources);
                let body = serde_json::to_string(&summaries).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/session_decisions" => {
                let raw_id = query.get("session_id").map(|s| s.as_str()).unwrap_or("");
                let session_id = percent_decode_str(raw_id).decode_utf8_lossy().to_string();
                let jsonl_path = query.get("jsonl_path").map(|s| {
                    percent_decode_str(s.as_str()).decode_utf8_lossy().to_string()
                });
                let resolved = if jsonl_path.is_none() {
                    scan_all_sources(&sources)
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

            "/messages" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let tail: Option<usize> = query.get("tail").and_then(|s| s.parse().ok());
                if let Some(source) = find_source_for_path(&sources, &file_path) {
                    let result = match tail {
                        Some(n) => source.get_messages_tail(&file_path, n),
                        None => source.get_messages(&file_path),
                    };
                    match result {
                        Ok(messages) => {
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

            "/file_size" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let uri = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let resolved = find_source_for_path(&sources, &uri)
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

            "/tail" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let uri = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                let offset: u64 = query
                    .get("offset")
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);

                let resolved = find_source_for_path(&sources, &uri)
                    .and_then(|s| s.resolve_file_path(&uri));
                let resolved = match resolved {
                    Some(p) => p,
                    None => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                        continue;
                    }
                };

                match std::fs::File::open(&resolved) {
                    Ok(mut file) => {
                        let file_size = file.metadata().map(|m| m.len()).unwrap_or(0);
                        if file_size <= offset {
                            let body = format!(r#"{{"lines":[],"newOffset":{}}}"#, offset);
                            let _ = request.respond(
                                tiny_http::Response::from_string(body).with_header(json_header),
                            );
                        } else {
                            let _ = file.seek(SeekFrom::Start(offset));
                            let mut buf = String::new();
                            let _ = file.read_to_string(&mut buf);
                            let lines: Vec<serde_json::Value> = buf
                                .lines()
                                .filter(|l| !l.trim().is_empty())
                                .filter_map(|l| serde_json::from_str(l).ok())
                                .collect();
                            let body = serde_json::json!({
                                "lines": lines,
                                "newOffset": file_size
                            })
                            .to_string();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body).with_header(json_header),
                            );
                        }
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

            "/memories" => {
                let mut memories = Vec::new();
                for source in &sources {
                    memories.extend(source.list_memories());
                }
                let body = serde_json::to_string(&memories).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/memory_content" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                // Try each source for memory content; fall back to direct read for Claude Code
                let result = sources.iter()
                    .find_map(|s| s.get_memory_content(&file_path).ok())
                    .or_else(|| memory::read_memory_file(&file_path).ok());
                match result {
                    Some(content) => {
                        let body = serde_json::to_string(&content).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    None => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

            "/memory_history" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                // Aggregate history from all sources; fall back to direct trace
                let mut history = Vec::new();
                for source in &sources {
                    let h = source.get_memory_history(&file_path);
                    if !h.is_empty() {
                        history = h;
                        break;
                    }
                }
                if history.is_empty() {
                    history = memory::trace_memory_history(&file_path);
                }
                let body = serde_json::to_string(&history).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/skills" => {
                let items = skills::scan_all_skills();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/plugins" => {
                let items = plugins::scan_with_catalog();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/plugins/set_enabled" if request.method() == &tiny_http::Method::Post => {
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

            "/plugins/marketplaces" if request.method() == &tiny_http::Method::Get => {
                let items =
                    crate::claude_cli::list_marketplaces().unwrap_or_default();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/plugins/marketplaces/add" if request.method() == &tiny_http::Method::Post => {
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

            "/plugins/marketplaces/remove" if request.method() == &tiny_http::Method::Post => {
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

            "/plugins/install" | "/plugins/uninstall"
                if request.method() == &tiny_http::Method::Post =>
            {
                #[derive(serde::Deserialize)]
                struct Body {
                    plugin_id: String,
                }
                let is_install = request.url().starts_with("/plugins/install");
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

            "/projects" if request.method() == &tiny_http::Method::Get => {
                let items = crate::project::list_projects();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/projects/create" if request.method() == &tiny_http::Method::Post => {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<crate::project::ProjectInput, _> =
                    serde_json::from_str(&buf);
                match parsed {
                    Ok(input) => match crate::project::create_project(input) {
                        Ok(p) => {
                            let body = serde_json::to_string(&p).unwrap_or_default();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body).with_header(json_header),
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

            "/projects/update" if request.method() == &tiny_http::Method::Post => {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<crate::project::Project, _> =
                    serde_json::from_str(&buf);
                match parsed {
                    Ok(project) => match crate::project::update_project(project) {
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

            "/projects/delete" if request.method() == &tiny_http::Method::Post => {
                #[derive(serde::Deserialize)]
                struct Body {
                    project_id: String,
                }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<Body, _> = serde_json::from_str(&buf);
                match parsed {
                    Ok(body) => match crate::project::delete_project(&body.project_id) {
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

            // ── Tasks (task-as-unit V1) ──────────────────────────────────────
            "/tasks/list" if request.method() == &tiny_http::Method::Get => {
                let project_filter = query.get("project_id").map(String::as_str);
                let items = crate::task::list_tasks(project_filter);
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/tasks/get" if request.method() == &tiny_http::Method::Get => {
                let id = query.get("id").cloned().unwrap_or_default();
                if id.is_empty() {
                    let _ = request.respond(
                        tiny_http::Response::from_string(r#"{"error":"missing id"}"#)
                            .with_status_code(400)
                            .with_header(json_header),
                    );
                    continue;
                }
                match crate::task::get_task(&id) {
                    Ok(t) => {
                        let body = serde_json::to_string(&t).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = serde_json::json!({ "error": e }).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(404)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/tasks/create" if request.method() == &tiny_http::Method::Post => {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<crate::task::TaskInput, _> =
                    serde_json::from_str(&buf);
                match parsed {
                    Ok(input) => match crate::task::create_task(input) {
                        Ok(t) => {
                            let body = serde_json::to_string(&t).unwrap_or_default();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body).with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = serde_json::json!({ "error": e }).to_string();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body)
                                    .with_status_code(500)
                                    .with_header(json_header),
                            );
                        }
                    },
                    Err(e) => {
                        let body = serde_json::json!({ "error": e.to_string() }).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/tasks/update-plan" if request.method() == &tiny_http::Method::Post => {
                #[derive(serde::Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Body {
                    task_id: String,
                    plan: crate::plan::DagPlan,
                }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<Body, _> = serde_json::from_str(&buf);
                match parsed {
                    Ok(b) => match crate::task::update_plan(&b.task_id, b.plan) {
                        Ok(()) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(r#"{"ok":true}"#)
                                    .with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = serde_json::json!({ "error": e }).to_string();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body)
                                    .with_status_code(500)
                                    .with_header(json_header),
                            );
                        }
                    },
                    Err(e) => {
                        let body = serde_json::json!({ "error": e.to_string() }).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/tasks/start" if request.method() == &tiny_http::Method::Post => {
                #[derive(serde::Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Body {
                    task_id: String,
                }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<Body, _> = serde_json::from_str(&buf);
                match parsed {
                    Ok(b) => match crate::task::start_task(&b.task_id) {
                        Ok(()) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(r#"{"ok":true}"#)
                                    .with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = serde_json::json!({ "error": e }).to_string();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body)
                                    .with_status_code(500)
                                    .with_header(json_header),
                            );
                        }
                    },
                    Err(e) => {
                        let body = serde_json::json!({ "error": e.to_string() }).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/tasks/set-title" if request.method() == &tiny_http::Method::Post => {
                #[derive(serde::Deserialize)]
                #[serde(rename_all = "camelCase")]
                struct Body {
                    task_id: String,
                    title: String,
                }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<Body, _> = serde_json::from_str(&buf);
                match parsed {
                    Ok(b) => match crate::task::set_task_title(&b.task_id, &b.title, false) {
                        Ok(()) => {
                            let _ = request.respond(
                                tiny_http::Response::from_string(r#"{"ok":true}"#)
                                    .with_header(json_header),
                            );
                        }
                        Err(e) => {
                            let body = serde_json::json!({ "error": e }).to_string();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body)
                                    .with_status_code(500)
                                    .with_header(json_header),
                            );
                        }
                    },
                    Err(e) => {
                        let body = serde_json::json!({ "error": e.to_string() }).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/tasks/upload-material" if request.method() == &tiny_http::Method::Post => {
                let task_id = match query.get("task_id") {
                    Some(s) if !s.is_empty() => s.clone(),
                    _ => {
                        let body =
                            serde_json::json!({"error": "missing task_id"}).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                        continue;
                    }
                };
                let raw_name = query.get("name").map(|s| s.as_str()).unwrap_or("");
                let decoded =
                    percent_decode_str(raw_name).decode_utf8_lossy().to_string();
                let safe_name: String = std::path::Path::new(&decoded)
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "material.bin".to_string());
                let media_str = query
                    .get("media")
                    .cloned()
                    .unwrap_or_else(|| "document".to_string());
                let media: crate::task::MediaKind = match serde_json::from_value(
                    serde_json::Value::String(media_str.clone()),
                ) {
                    Ok(m) => m,
                    Err(_) => {
                        let body = serde_json::json!({
                            "error": format!("invalid media kind: {media_str}")
                        })
                        .to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                        continue;
                    }
                };

                const MAX: u64 = crate::backend::MAX_ATTACHMENT_BYTES;
                if let Some(len) = request.body_length() {
                    if (len as u64) > MAX {
                        let body = serde_json::json!({
                            "error": format!("material too large: {len} bytes (max {MAX})")
                        })
                        .to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(413)
                                .with_header(json_header),
                        );
                        continue;
                    }
                }
                let mut body_bytes = Vec::new();
                let mut limited =
                    std::io::Read::take(request.as_reader(), MAX + 1);
                let _ =
                    std::io::Read::read_to_end(&mut limited, &mut body_bytes);
                if (body_bytes.len() as u64) > MAX {
                    let body = serde_json::json!({
                        "error": format!("material too large: >{MAX} bytes")
                    })
                    .to_string();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_status_code(413)
                            .with_header(json_header),
                    );
                    continue;
                }

                match crate::task::add_task_material(
                    &task_id,
                    &safe_name,
                    &body_bytes,
                    media,
                ) {
                    Ok(path) => {
                        let body = serde_json::json!({
                            "path": path.to_string_lossy()
                        })
                        .to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
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

            "/tasks/events" if request.method() == &tiny_http::Method::Get => {
                // V1 stub: returns the task id as the subscription handle. The
                // master runtime (P19) + event router (P21) hook in here to
                // push semantic TaskEvent values over SSE. For now: validate
                // the task exists, return an `ok` so RemoteBackend can finish
                // its handshake.
                let id = query.get("id").cloned().unwrap_or_default();
                if id.is_empty() {
                    let _ = request.respond(
                        tiny_http::Response::from_string(r#"{"error":"missing id"}"#)
                            .with_status_code(400)
                            .with_header(json_header),
                    );
                    continue;
                }
                match crate::task::get_task(&id) {
                    Ok(_) => {
                        let body = serde_json::json!({ "subscriptionId": id }).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = serde_json::json!({ "error": e }).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(404)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/fleet_sessions" if request.method() == &tiny_http::Method::Get => {
                let items = crate::project::list_fleet_sessions();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/fleet_sessions/needing-input" if request.method() == &tiny_http::Method::Get => {
                let items = crate::supervisor::session_pending_requests();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/fleet_sessions/spawn" if request.method() == &tiny_http::Method::Post => {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                let parsed: Result<crate::project::LauncherForm, _> =
                    serde_json::from_str(&buf);
                match parsed {
                    Ok(form) => match crate::supervisor::enqueue(form) {
                        Ok(s) => {
                            let body = serde_json::to_string(&s).unwrap_or_default();
                            let _ = request.respond(
                                tiny_http::Response::from_string(body).with_header(json_header),
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

            "/fleet_sessions/cancel" if request.method() == &tiny_http::Method::Post => {
                #[derive(serde::Deserialize)]
                struct Body { session_id: String }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                match serde_json::from_str::<Body>(&buf) {
                    Ok(b) => match crate::supervisor::cancel(&b.session_id) {
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

            "/fleet_sessions/resume" if request.method() == &tiny_http::Method::Post => {
                #[derive(serde::Deserialize)]
                struct Body { session_id: String, follow_up_prompt: String }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                match serde_json::from_str::<Body>(&buf) {
                    Ok(b) => match crate::supervisor::resume(&b.session_id, b.follow_up_prompt) {
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

            "/fleet_sessions/status" if request.method() == &tiny_http::Method::Post => {
                #[derive(serde::Deserialize)]
                struct Body {
                    session_id: String,
                    status: String,
                    note: Option<String>,
                }
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                match serde_json::from_str::<Body>(&buf) {
                    Ok(b) => match crate::supervisor::set_status(
                        &b.session_id,
                        &b.status,
                        b.note,
                    ) {
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

            "/skill_history" => {
                let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
                let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
                if let Some(source) = find_source_for_path(&sources, &file_path) {
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

            "/token_breakdown" => {
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

            "/skill_content" => {
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

            "/skill_files" => {
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

            "/skill_delete" if request.method() == &tiny_http::Method::Post => {
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

            "/hooks_plan" => {
                let plan = hooks::plan_hook_setup();
                let body = serde_json::to_string(&plan).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/apply_hooks" => {
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

            "/sources_config" => {
                let info = agent_source::get_sources_config_local();
                let body = serde_json::to_string(&info).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/set_source_enabled" => {
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

            "/list_claude_binaries" => {
                let bins = crate::claude_binary::discover();
                let body = serde_json::to_string(&bins).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/claude_binary_override" => {
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

            "/remove_hooks" => {
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

            // ── Guard hook endpoints ──────────────────────────────────────
            "/apply_guard_hook" => {
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

            "/remove_guard_hook" => {
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

            "/guard/pending" => {
                let ids = guard::list_pending_requests();
                let sessions = scan_all_sources(&sources);
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

            "/guard/respond" => {
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

            "/guard/analyze" => {
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
                        let provider = llm_provider::resolve_provider(&cfg.provider);
                        let result = provider.as_ref().and_then(|p| {
                            if !p.is_available() { return None; }
                            let model = &cfg.fast_model;
                            let timeout = std::time::Duration::from_secs(30);
                            crate::llm_usage::complete_accounted(
                                p.as_ref(),
                                &prompt,
                                model,
                                timeout,
                                crate::llm_usage::SCENARIO_GUARD_COMMAND,
                            )
                        });
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

            "/guard/allow-rules" => {
                let rules = audit::list_guard_allow_rules();
                let body = serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/guard/allow-rules/remove" => {
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

            // ── Elicitation hook endpoints ───────────────────────────────────
            "/apply_elicitation_hook" => {
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

            "/remove_elicitation_hook" => {
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

            "/elicitation/pending" => {
                let ids = elicitation::list_pending_requests();
                let sessions = scan_all_sources(&sources);
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
                let body = serde_json::to_string(&requests).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            // ── Interaction mode endpoints ───────────────────────────────────
            "/apply_interaction_mode" => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct Req { user_title: String, locale: String }
                match serde_json::from_slice::<Req>(&body_bytes) {
                    Ok(req_body) => {
                        match interaction_mode::apply_interaction_mode(&req_body.user_title, &req_body.locale) {
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

            "/remove_interaction_mode" => {
                match interaction_mode::remove_interaction_mode() {
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

            // ── PRD Discipline mode endpoints ────────────────────────────────
            "/apply_prd_mode" => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct Req { user_title: String, locale: String }
                match serde_json::from_slice::<Req>(&body_bytes) {
                    Ok(req_body) => {
                        let result = prd_discipline::apply_prd_discipline(
                            &req_body.user_title,
                            &req_body.locale,
                        )
                        .and_then(|()| hooks::apply_prd_context_hook());
                        match result {
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

            "/remove_prd_mode" => {
                let r1 = prd_discipline::remove_prd_discipline();
                let r2 = hooks::remove_prd_context_hook();
                match r1.and(r2) {
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

            // ── Plan-approval hook endpoints ─────────────────────────────────
            "/apply_plan_approval_hook" => {
                match hooks::apply_plan_approval_hook() {
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

            "/remove_plan_approval_hook" => {
                match hooks::remove_plan_approval_hook() {
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

            "/plan-approval/pending" => {
                let ids = plan_approval::list_pending_requests();
                let sessions = scan_all_sources(&sources);
                let mut requests = Vec::new();
                for id in &ids {
                    if let Some(mut req) = plan_approval::read_request(id) {
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

            "/plan-approval/respond" => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<plan_approval::PlanApprovalResponse>(&body_bytes) {
                    Ok(resp) => {
                        match plan_approval::write_response(&resp) {
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

            "/elicitation/respond" => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<elicitation::ElicitationResponse>(&body_bytes) {
                    Ok(resp) => {
                        match elicitation::write_response(&resp) {
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

            "/elicitation/upload" => {
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
                        continue;
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
                    continue;
                }

                let dir = std::env::temp_dir().join("fleet-attachments");
                if let Err(e) = std::fs::create_dir_all(&dir) {
                    let body = serde_json::json!({"error": format!("mkdir: {e}")}).to_string();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_status_code(500)
                            .with_header(json_header),
                    );
                    continue;
                }

                let nanos = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_nanos();
                let pid = std::process::id();
                let filename = format!("{nanos}-{pid}-{safe_name}");
                let dest = dir.join(&filename);

                if let Err(e) = std::fs::write(&dest, &body_bytes) {
                    let body = serde_json::json!({"error": format!("write: {e}")}).to_string();
                    let _ = request.respond(
                        tiny_http::Response::from_string(body)
                            .with_status_code(500)
                            .with_header(json_header),
                    );
                    continue;
                }

                let abs = dest.to_string_lossy().into_owned();
                let body = serde_json::json!({"path": abs}).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/search" => {
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

            "/audit" => {
                use crate::audit::extract_audit_events;
                let sessions = scan_all_sources(&sources);
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
                    if let Some(src) = find_source_for_path(&sources, path) {
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
                    if let Some(src) = find_source_for_path(&sources, path) {
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

            "/audit/pattern-info" => {
                let (version, path) = crate::pattern_update::get_patterns_info();
                let body = serde_json::json!({
                    "version": version,
                    "path": path,
                }).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/audit/check-update" => {
                let msg = crate::pattern_update::check_update_now();
                let body = serde_json::json!({ "message": msg }).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/audit/rules" => {
                let rules = crate::audit::get_all_rules();
                let body = serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/audit/rules/toggle" => {
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

            "/audit/rules/save" => {
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

            "/audit/rules/delete" => {
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

            "/audit/rules/suggest" => {
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

            "/daily_report" => {
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

            "/daily_report_stats" => {
                let from = query.get("from").cloned().unwrap_or_default();
                let to = query.get("to").cloned().unwrap_or_default();
                let store = report_store.lock().unwrap();
                let stats = store.list_stats(&from, &to).unwrap_or_default();
                let body = serde_json::to_string(&stats).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/daily_report/generate" => {
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

            "/daily_report/ai_summary" => {
                let date = query.get("date").cloned().unwrap_or_default();
                let lang = query.get("lang").map(|s| s.as_str()).unwrap_or("en");
                let store = report_store.lock().unwrap();
                match store.get_report(&date) {
                    Ok(Some(report)) => {
                        drop(store);
                        let cfg = llm_config.lock().unwrap().clone();
                        let provider = llm_provider::resolve_provider(&cfg.provider);
                        let result = provider.as_ref().and_then(|p| {
                            generate_ai_summary(p.as_ref(), &cfg.standard_model, &report, lang)
                        });
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

            "/daily_report/lessons" => {
                let date = query.get("date").cloned().unwrap_or_default();
                let lang = query.get("lang").map(|s| s.as_str()).unwrap_or("en");
                let store = report_store.lock().unwrap();
                match store.get_report(&date) {
                    Ok(Some(report)) => {
                        drop(store);
                        let cfg = llm_config.lock().unwrap().clone();
                        let provider = llm_provider::resolve_provider(&cfg.provider);
                        let result = provider.as_ref().and_then(|p| {
                            generate_lessons(p.as_ref(), &cfg.standard_model, &report, lang)
                        });
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

            "/daily_report/append_lesson" => {
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

            // ── LLM provider endpoints ──────────────────────────────────
            "/llm/providers" => {
                let infos = llm_provider::all_provider_infos();
                let body = serde_json::to_string(&infos).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/llm/config" if request.method() == &tiny_http::Method::Get => {
                let cfg = llm_config.lock().unwrap().clone();
                let body = serde_json::to_string(&cfg).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/llm/config" => {
                // POST: update config
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<LlmConfig>(&body_bytes) {
                    Ok(new_cfg) => {
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

            "/fleet_llm_usage/daily" => {
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

            // ── Session outcome analysis (delegated from remote clients) ──
            "/analyze" => {
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

            // ── Feishu (Lark) bridge endpoints ───────────────────────────
            "/feishu/start_oauth" => {
                let body = match crate::feishu::start_oauth() {
                    Ok(handle) => serde_json::to_string(&handle).unwrap_or_default(),
                    Err(e) => {
                        let body = serde_json::json!({"error": e}).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(503)
                                .with_header(json_header),
                        );
                        continue;
                    }
                };
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/feishu/poll_oauth" => {
                let qs = parse_query(query_str);
                let state = qs.get("state").cloned().unwrap_or_default();
                match crate::feishu::poll_oauth(&state) {
                    Ok(status) => {
                        let body = serde_json::to_string(&status).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(e) => {
                        let body = serde_json::json!({"error": e}).to_string();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body)
                                .with_status_code(404)
                                .with_header(json_header),
                        );
                    }
                }
            }

            "/feishu/status" => {
                match crate::feishu::status() {
                    Ok(conn) => {
                        let body = serde_json::to_string(&conn).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
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

            "/feishu/disconnect" => {
                match crate::feishu::disconnect() {
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

            "/feishu/creds" if request.method() == &tiny_http::Method::Get => {
                let creds = crate::feishu::get_stored_creds();
                let body = serde_json::to_string(&creds).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            "/feishu/creds" if request.method() == &tiny_http::Method::Post => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                let parsed: Result<crate::feishu::StoredCreds, _> =
                    serde_json::from_slice(&body_bytes);
                let (status, body) = match parsed {
                    Ok(c) => match crate::feishu::set_stored_creds(c) {
                        Ok(()) => (200, r#"{"ok":true}"#.to_string()),
                        Err(e) => (500, serde_json::json!({"error": e}).to_string()),
                    },
                    Err(e) => (
                        400,
                        serde_json::json!({"error": format!("invalid body: {e}")}).to_string(),
                    ),
                };
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(status)
                        .with_header(json_header),
                );
            }

            "/webhook/feishu" if request.method() == &tiny_http::Method::Post => {
                let timestamp = request
                    .headers()
                    .iter()
                    .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("X-Lark-Request-Timestamp"))
                    .map(|h| h.value.as_str().to_string());
                let nonce = request
                    .headers()
                    .iter()
                    .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("X-Lark-Request-Nonce"))
                    .map(|h| h.value.as_str().to_string());
                let signature = request
                    .headers()
                    .iter()
                    .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case("X-Lark-Signature"))
                    .map(|h| h.value.as_str().to_string());

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);

                let (status, body) = match crate::feishu::handle_webhook(
                    &body_bytes,
                    timestamp.as_deref(),
                    nonce.as_deref(),
                    signature.as_deref(),
                ) {
                    Ok(b) => (200, b),
                    Err(e) => (
                        400,
                        serde_json::json!({"error": e}).to_string().into_bytes(),
                    ),
                };
                let _ = request.respond(
                    tiny_http::Response::from_data(body)
                        .with_status_code(status)
                        .with_header(json_header),
                );
            }

            _ => {
                let _ = request.respond(tiny_http::Response::empty(404));
            }
        }
    }
}


fn parse_query(query_str: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    for pair in query_str.split('&') {
        if pair.is_empty() {
            continue;
        }
        match pair.split_once('=') {
            Some((k, v)) => {
                map.insert(k.to_string(), v.to_string());
            }
            None => {
                map.insert(pair.to_string(), String::new());
            }
        }
    }
    map
}
