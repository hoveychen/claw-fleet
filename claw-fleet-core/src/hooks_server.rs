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

    // Inject Fleet's permissions allowlist into ~/.claude/settings.json so
    // fleet guard is the sole audit gate for this serve process. The matching
    // release() is wired to SIGINT/SIGTERM below; a `kill -9` skips it, and
    // the next Fleet startup's prune_dead_holders takes care of the stale pid.
    // The release path is unconditional even when the toggle is off — it's a
    // no-op when no lock exists, so it self-heals if the user flipped the
    // toggle off mid-run after we'd already acquired.
    //
    // release() only deregisters the pid; the allowlist stays in settings.json
    // so detached claude sessions survive this process. Un-injecting happens
    // solely via permissions_injector::deactivate() (the settings-panel toggle).
    let serve_pid = std::process::id();
    if crate::permissions_injector::load_config().enabled {
        if let Err(e) = crate::permissions_injector::acquire(serve_pid) {
            eprintln!("[fleet serve] permissions_injector::acquire failed: {e}");
        }
    }
    // Mirror the permission injection for the MCP server registration. We are
    // the fleet binary, so current_exe() is the right `command` to publish.
    //
    // Debug-only gate (matches the desktop side in gui.rs): v2 fleet__ask has
    // UX gaps vs v1 AskUserQuestion that we want fixed before release users
    // get the tool by default. Release builds skip injection and release()
    // any stale entry left over by an earlier dev install.
    if cfg!(debug_assertions) && crate::mcp_injector::load_config().enabled {
        match std::env::current_exe() {
            Ok(p) => {
                let path_str = p.to_string_lossy().to_string();
                if let Err(e) = crate::mcp_injector::acquire(serve_pid, &path_str) {
                    eprintln!("[fleet serve] mcp_injector::acquire failed: {e}");
                }
            }
            Err(e) => eprintln!("[fleet serve] current_exe failed, skipping mcp_injector: {e}"),
        }
    } else if !cfg!(debug_assertions) {
        let _ = crate::mcp_injector::release(serve_pid);
    }
    if let Err(e) = ctrlc::try_set_handler(move || {
        let _ = crate::permissions_injector::release(serve_pid);
        let _ = crate::mcp_injector::release(serve_pid);
        std::process::exit(0);
    }) {
        eprintln!("[fleet serve] ctrlc handler install failed: {e}");
    }

    // Drift watchdog: every 30s, verify both injectors still own their
    // expected regions of ~/.claude.json and ~/.claude/settings.json. If
    // a third party (e.g. a CC upgrade) rewrote those files, the
    // watchdog re-injects. No-ops when no holders are live or the
    // per-injector toggle is off. Thread runs until process exit.
    let watchdog_fleet_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_else(|_| "fleet".to_string());
    crate::injector_watchdog::start(watchdog_fleet_path);

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

    // ── Usage occupancy sampler ─────────────────────────────────────────────
    // Sample the Claude usage API every 10 minutes so the 24h occupancy chart
    // has continuous coverage even when no desktop UI is actively polling. This
    // is the always-on host for remote setups (the probe machine's usage) and
    // for local users who run `fleet serve`. Idempotent (once-per-process) and
    // self-healing — offline / not-logged-in errors are swallowed and retried.
    crate::account::start_background_sampler(std::time::Duration::from_secs(600));

    // Bring up the mobile relay channel if it's configured — on a probe-only
    // machine (no desktop app) this serve process is the agent-side publisher
    // for the mobile web (see the broadcast loop below).
    crate::mobile_relay::ensure_ws_client();

    // ── Background SSE broadcaster thread ──────────────────────────────────
    // Polls for session changes, waiting alerts, guard/elicitation requests
    // and pushes them to connected SSE clients.
    {
        let sse_bg = sse.clone();
        std::thread::spawn(move || {
            let sources_bg = build_sources();
            let mut prev_sessions_json = String::new();
            let mut prev_mobile_clients: usize = 0;
            let mut prev_alert_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut prev_guard_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut prev_elicit_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut prev_plan_approval_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut prev_fleet_ask_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut prev_a2ui_render_ids: std::collections::HashSet<String> = std::collections::HashSet::new();
            let mut prev_permission_prompt_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

            loop {
                std::thread::sleep(std::time::Duration::from_secs(2));

                // Mobile relay clients count as consumers too: with no SSE
                // desktop attached but a phone online, the loop must keep
                // scanning so decisions still reach the mobile web.
                if sse_bg.client_count() == 0 && !crate::mobile_relay::is_connected() {
                    continue;
                }

                // A SSE client is connected — mark this serve as a live consumer
                // so `fleet guard`/`fleet elicitation` know the head is reachable.
                crate::consumer_heartbeat::write_heartbeat();

                // Broadcast session updates
                let sessions = scan_all_sources(&sources_bg);
                let json = serde_json::to_string(&sessions).unwrap_or_default();
                let sessions_changed = json != prev_sessions_json;
                if sessions_changed {
                    sse_bg.broadcast("sessions-updated", &json);
                }
                // Publish to mobile on change, and also when a new mobile
                // client just came online (it needs an initial snapshot even
                // if nothing changed since the last one).
                let mobile_clients = crate::mobile_relay::client_count();
                if sessions_changed || mobile_clients > prev_mobile_clients {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&json) {
                        crate::mobile_relay::publish_sessions(&v);
                    }
                }
                prev_mobile_clients = mobile_clients;
                if sessions_changed {
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
                            if let Ok(v) = serde_json::to_value(&req) {
                                crate::mobile_relay::publish_decision_created(
                                    "guard",
                                    v,
                                    &format!(
                                        "{} · 命令待审批",
                                        crate::mobile_relay::notify_workspace(&req.workspace_name)
                                    ),
                                    &crate::mobile_relay::notify_preview(&req.command_summary),
                                );
                            }
                        }
                    }
                }
                // Broadcast dismissed guard requests (answered / cleaned up)
                for id in prev_guard_ids.difference(&guard_ids) {
                    if let Ok(json) = serde_json::to_string(id) {
                        sse_bg.broadcast("guard-dismissed", &json);
                    }
                    crate::mobile_relay::publish_decision_resolved("guard", id);
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
                            if let Ok(v) = serde_json::to_value(&req) {
                                crate::mobile_relay::publish_decision_created(
                                    "elicitation",
                                    v,
                                    &format!(
                                        "{} · 有问题请示",
                                        crate::mobile_relay::notify_workspace(&req.workspace_name)
                                    ),
                                    &crate::mobile_relay::notify_preview(
                                        req.questions.first().map(|q| q.question.as_str()).unwrap_or(""),
                                    ),
                                );
                            }
                        }
                    }
                }
                // Broadcast dismissed elicitation requests (answered / cleaned up)
                for id in prev_elicit_ids.difference(&elicit_ids) {
                    if let Ok(json) = serde_json::to_string(id) {
                        sse_bg.broadcast("elicitation-dismissed", &json);
                    }
                    crate::mobile_relay::publish_decision_resolved("elicitation", id);
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
                            if let Ok(v) = serde_json::to_value(&req) {
                                crate::mobile_relay::publish_decision_created(
                                    "plan-approval",
                                    v,
                                    &format!(
                                        "{} · 计划待审批",
                                        crate::mobile_relay::notify_workspace(&req.workspace_name)
                                    ),
                                    &crate::mobile_relay::notify_preview(
                                        req.ai_title.as_deref().unwrap_or("ExitPlanMode 计划审批"),
                                    ),
                                );
                            }
                        }
                    }
                }
                // Broadcast dismissed plan-approval requests (answered / cleaned up)
                for id in prev_plan_approval_ids.difference(&plan_approval_ids) {
                    if let Ok(json) = serde_json::to_string(id) {
                        sse_bg.broadcast("plan-approval-dismissed", &json);
                    }
                    crate::mobile_relay::publish_decision_resolved("plan-approval", id);
                }
                prev_plan_approval_ids.retain(|id| plan_approval_ids.contains(id));

                // Broadcast new fleet__ask requests (MCP tool bridge)
                let fleet_ask_ids: std::collections::HashSet<String> =
                    crate::mcp_ipc::list_pending_requests().into_iter().collect();
                for id in &fleet_ask_ids {
                    if prev_fleet_ask_ids.insert(id.clone()) {
                        if let Some(mut req) = crate::mcp_ipc::read_request(id) {
                            if let Some(s) = sessions.iter().find(|s| s.id == req.session_id) {
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = s.workspace_name.clone();
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = s.ai_title.clone();
                                }
                            }
                            if let Ok(json) = serde_json::to_string(&req) {
                                sse_bg.broadcast("fleet-ask-request", &json);
                            }
                            if let Ok(v) = serde_json::to_value(&req) {
                                crate::mobile_relay::publish_decision_created(
                                    "fleet-ask",
                                    v,
                                    &format!(
                                        "{} · 决策卡待处理",
                                        crate::mobile_relay::notify_workspace(&req.workspace_name)
                                    ),
                                    &crate::mobile_relay::notify_preview(
                                        req.ai_title.as_deref().unwrap_or_else(|| {
                                            req.questions
                                                .first()
                                                .map(|q| q.question.as_str())
                                                .unwrap_or("")
                                        }),
                                    ),
                                );
                            }
                        }
                    }
                }
                for id in prev_fleet_ask_ids.difference(&fleet_ask_ids) {
                    if let Ok(json) = serde_json::to_string(id) {
                        sse_bg.broadcast("fleet-ask-dismissed", &json);
                    }
                    crate::mobile_relay::publish_decision_resolved("fleet-ask", id);
                }
                prev_fleet_ask_ids.retain(|id| fleet_ask_ids.contains(id));

                // Broadcast new fleet__render_a2ui requests (parallel MCP tool)
                let a2ui_render_ids: std::collections::HashSet<String> =
                    crate::mcp_a2ui_ipc::list_pending_requests().into_iter().collect();
                for id in &a2ui_render_ids {
                    if prev_a2ui_render_ids.insert(id.clone()) {
                        if let Some(mut req) = crate::mcp_a2ui_ipc::read_request(id) {
                            if let Some(s) = sessions.iter().find(|s| s.id == req.session_id) {
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = s.workspace_name.clone();
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = s.ai_title.clone();
                                }
                            }
                            if let Ok(json) = serde_json::to_string(&req) {
                                sse_bg.broadcast("a2ui-render-request", &json);
                            }
                        }
                    }
                }
                for id in prev_a2ui_render_ids.difference(&a2ui_render_ids) {
                    if let Ok(json) = serde_json::to_string(id) {
                        sse_bg.broadcast("a2ui-render-dismissed", &json);
                    }
                }
                prev_a2ui_render_ids.retain(|id| a2ui_render_ids.contains(id));

                // Broadcast new fleet__permission_prompt requests (headless
                // native-permission bridge via --permission-prompt-tool)
                let permission_prompt_ids: std::collections::HashSet<String> =
                    crate::permission_prompt_ipc::list_pending_requests().into_iter().collect();
                for id in &permission_prompt_ids {
                    if prev_permission_prompt_ids.insert(id.clone()) {
                        if let Some(mut req) = crate::permission_prompt_ipc::read_request(id) {
                            if let Some(s) = sessions.iter().find(|s| s.id == req.session_id) {
                                if req.workspace_name.is_empty() {
                                    req.workspace_name = s.workspace_name.clone();
                                }
                                if req.ai_title.is_none() {
                                    req.ai_title = s.ai_title.clone();
                                }
                            }
                            if let Ok(json) = serde_json::to_string(&req) {
                                sse_bg.broadcast("permission-prompt-request", &json);
                            }
                            if let Ok(v) = serde_json::to_value(&req) {
                                crate::mobile_relay::publish_decision_created(
                                    "permission-prompt",
                                    v,
                                    &format!(
                                        "{} · 权限请求",
                                        crate::mobile_relay::notify_workspace(&req.workspace_name)
                                    ),
                                    &crate::mobile_relay::notify_preview(&req.tool_name),
                                );
                            }
                        }
                    }
                }
                for id in prev_permission_prompt_ids.difference(&permission_prompt_ids) {
                    if let Ok(json) = serde_json::to_string(id) {
                        sse_bg.broadcast("permission-prompt-dismissed", &json);
                    }
                    crate::mobile_relay::publish_decision_resolved("permission-prompt", id);
                }
                prev_permission_prompt_ids.retain(|id| permission_prompt_ids.contains(id));
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
            crate::routes::HEALTH => {
                let body = format!(
                    r#"{{"version":"{}","status":"ok"}}"#,
                    env!("CARGO_PKG_VERSION")
                );
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::SESSIONS => {
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

            crate::routes::INTERRUPT => {
                let pid: u32 = query.get("pid").and_then(|s| s.parse().ok()).unwrap_or(0);
                if pid == 0 {
                    let _ = request.respond(tiny_http::Response::empty(400));
                    continue;
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

            crate::routes::STOP => {
                let pid: u32 = query.get("pid").and_then(|s| s.parse().ok()).unwrap_or(0);
                if pid == 0 {
                    let _ = request.respond(tiny_http::Response::empty(400));
                    continue;
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
                        continue;
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

            // `/stop_workspace` kills every agent process (and its tree) rooted
            // in a workspace. Retired in Phase 4 as a "lifecycle endpoint", but
            // RemoteBackend::kill_workspace never stopped calling it, so a remote
            // stop on an imprecise pid 404'd silently. Re-added to match the
            // still-live client. `?path=` is percent-encoded (slashes as %2F).
            crate::routes::STOP_WORKSPACE => {
                let path_param = query
                    .get("path")
                    .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                    .unwrap_or_default();
                if path_param.is_empty() {
                    let _ = request.respond(tiny_http::Response::empty(400));
                    continue;
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

            // Phase 4 P3: `/auto_resume_config` removed. Remote callers hitting
            // that path receive the catch-all 404 below. `/resume_session` was
            // re-added later (POST, with follow-up prompt) for the history
            // panel — see the handler further down.

            // ── Unified /sources/{name}/account and /sources/{name}/usage ──
            _ if path.starts_with(crate::routes::SOURCES_PREFIX) => {
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

            crate::routes::SETUP_STATUS => {
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

            crate::routes::USAGE_SUMMARIES => {
                let summaries = crate::agent_source::fetch_usage_summaries_from_sources(&sources);
                let body = serde_json::to_string(&summaries).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::TODAY_USAGE => {
                let sessions = scan_all_sources(&sources);
                let usage = crate::today_usage::today_usage(&sessions);
                let body = serde_json::to_string(&usage).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::SESSION_DECISIONS => {
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

            crate::routes::MESSAGES => {
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

            crate::routes::FILE_SIZE => {
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

            crate::routes::TAIL => {
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

            crate::routes::MEMORIES => {
                let mut memories = Vec::new();
                for source in &sources {
                    memories.extend(source.list_memories());
                }
                let body = serde_json::to_string(&memories).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::LIVE_THINKING => {
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

            crate::routes::MEMORY_CONTENT => {
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

            crate::routes::MEMORY_HISTORY => {
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

            crate::routes::HANDOFF_CHAIN => {
                let raw = query.get("session").map(|s| s.as_str()).unwrap_or("");
                let sid = percent_decode_str(raw).decode_utf8_lossy().to_string();
                let chain = crate::handoff::chain_containing(&sid);
                let body = serde_json::to_string(&chain).unwrap_or_default();
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_header(json_header));
            }

            crate::routes::WIKI_DOCS => {
                let body = serde_json::to_string(&crate::wiki::list_docs()).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::WIKI_DOC => {
                let raw = query.get("slug").map(|s| s.as_str()).unwrap_or("");
                let slug = percent_decode_str(raw).decode_utf8_lossy().to_string();
                match crate::wiki::get_doc(&slug) {
                    Ok(doc) => {
                        let body = serde_json::to_string(&doc).unwrap_or_default();
                        let _ = request.respond(
                            tiny_http::Response::from_string(body).with_header(json_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

            crate::routes::WIKI_FILE => {
                let dec = |key: &str| {
                    query
                        .get(key)
                        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                        .unwrap_or_default()
                };
                let (slug, version, rel) = (dec("slug"), dec("version"), dec("path"));
                match crate::wiki::get_file(&slug, &version, &rel) {
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

            crate::routes::DECISION_ASSET => {
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

            crate::routes::WIKI_SEARCH => {
                let raw = query.get("q").map(|s| s.as_str()).unwrap_or("");
                let q = percent_decode_str(raw).decode_utf8_lossy().to_string();
                let body =
                    serde_json::to_string(&crate::wiki::search_docs(&q)).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::WIKI_EXPORT => {
                let dec = |key: &str| {
                    query
                        .get(key)
                        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                        .unwrap_or_default()
                };
                match crate::wiki::export_doc(&dec("slug"), &dec("version")) {
                    Ok(e) => {
                        let mime_header: tiny_http::Header =
                            format!("Content-Type: {}", e.mime).parse().unwrap();
                        let _ = request.respond(
                            tiny_http::Response::from_data(e.bytes).with_header(mime_header),
                        );
                    }
                    Err(_) => {
                        let _ = request.respond(tiny_http::Response::empty(404));
                    }
                }
            }

            crate::routes::WIKI_DELETE if request.method() == &tiny_http::Method::Post => {
                let dec = |key: &str| {
                    query
                        .get(key)
                        .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                        .unwrap_or_default()
                };
                let (slug, version) = (dec("slug"), dec("version"));
                let result = if version.is_empty() {
                    crate::wiki::delete_doc(&slug)
                } else {
                    crate::wiki::delete_version(&slug, &version)
                };
                match result {
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
                                .with_status_code(400)
                                .with_header(json_header),
                        );
                    }
                }
            }

            crate::routes::WIKI_MOVE if request.method() == &tiny_http::Method::Post => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct Req {
                    from: String,
                    to: String,
                }
                let moved = serde_json::from_slice::<Req>(&body_bytes)
                    .map_err(|e| format!("bad /wiki_move body: {e}"))
                    .and_then(|r| crate::wiki::move_doc(&r.from, &r.to));
                match moved {
                    Ok(doc) => {
                        let body = serde_json::to_string(&doc).unwrap_or_default();
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

            // Folder ops. `to: ""` dissolves the folder into the tree root.
            crate::routes::WIKI_MOVE_FOLDER if request.method() == &tiny_http::Method::Post => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct Req {
                    from: String,
                    to: String,
                }
                let moved = serde_json::from_slice::<Req>(&body_bytes)
                    .map_err(|e| format!("bad /wiki_move_folder body: {e}"))
                    .and_then(|r| crate::wiki::move_folder(&r.from, &r.to));
                match moved {
                    Ok(docs) => {
                        let body = serde_json::to_string(&docs).unwrap_or_default();
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

            crate::routes::WIKI_DELETE_FOLDER if request.method() == &tiny_http::Method::Post => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct Req {
                    prefix: String,
                }
                let deleted = serde_json::from_slice::<Req>(&body_bytes)
                    .map_err(|e| format!("bad /wiki_delete_folder body: {e}"))
                    .and_then(|r| crate::wiki::delete_folder(&r.prefix));
                match deleted {
                    Ok(count) => {
                        let body = serde_json::json!({ "deleted": count }).to_string();
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

            crate::routes::TASK_PLANS => {
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

            crate::routes::SKILLS => {
                let items = skills::scan_all_skills();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::PLUGINS => {
                let items = plugins::scan_with_catalog();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::PLUGINS_SET_ENABLED
                if request.method() == &tiny_http::Method::Post =>
            {
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

            crate::routes::PLUGINS_MARKETPLACES if request.method() == &tiny_http::Method::Get => {
                let items =
                    crate::claude_cli::list_marketplaces().unwrap_or_default();
                let body = serde_json::to_string(&items).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::PLUGINS_MARKETPLACES_ADD if request.method() == &tiny_http::Method::Post => {
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

            crate::routes::PLUGINS_MARKETPLACES_REMOVE if request.method() == &tiny_http::Method::Post => {
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

            crate::routes::PLUGINS_INSTALL | crate::routes::PLUGINS_UNINSTALL
                if request.method() == &tiny_http::Method::Post =>
            {
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

            // Resume a scanned session with an optional follow-up prompt
            // (history panel's "恢复会话", remote backend). Detached
            // `claude --resume <sid> -p <prompt>`; the resumed turn appears
            // via the scanner as the JSONL grows.
            crate::routes::RESUME_SESSION if request.method() == &tiny_http::Method::Post => {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                match serde_json::from_str::<crate::auto_resume::ResumeSessionRequest>(&buf) {
                    Ok(req) => {
                        match crate::auto_resume::spawn_resume_prompt(
                            &req.session_id,
                            &req.workspace_path,
                            req.prompt.as_deref().unwrap_or("continue"),
                            req.model.as_deref(),
                            req.effort.as_deref(),
                            req.permission_mode.as_deref(),
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

            // Absolute path of this host's pure-chat workspace, created on
            // demand. The remote desktop pins it in its launcher but cannot
            // derive it — the path is under *this* host's home.
            crate::routes::CHAT_WORKSPACE if request.method() == &tiny_http::Method::Get => {
                match crate::chat_workspace::ensure_chat_workspace() {
                    Ok(path) => {
                        let body = serde_json::json!({"path": path}).to_string();
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

            // Directory picker for the remote desktop's launcher. Same reason
            // /chat_workspace exists: the directory the user is choosing lives on
            // *this* host, and the desktop's native OS dialog can only browse its
            // own disk. `path` absent = this host's home.
            //
            // Not gated on known session workspaces — picking a directory that has
            // never had a session is the entire point. `workspace_browse` carries
            // its own boundary (home + off-home known workspaces, canonical).
            crate::routes::BROWSE_DIR if request.method() == &tiny_http::Method::Get => {
                let path = query
                    .get("path")
                    .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                    .filter(|s| !s.is_empty());
                let known: Vec<String> = sources
                    .iter()
                    .flat_map(|s| s.scan_sessions())
                    .map(|s| s.workspace_path)
                    .collect();
                match crate::workspace_browse::browse_dir(path.as_deref(), &known) {
                    Ok(resp) => {
                        let body = serde_json::to_string(&resp).unwrap_or_default();
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

            // Spawn a brand-new headless Claude Code session (sessions page's
            // "new session" button, remote backend). Detached `claude -p`;
            // the session appears via the scanner once its JSONL exists.
            crate::routes::SPAWN_SESSION if request.method() == &tiny_http::Method::Post => {
                let mut buf = String::new();
                let _ = std::io::Read::read_to_string(request.as_reader(), &mut buf);
                match serde_json::from_str::<crate::session_launch::SpawnSessionRequest>(&buf) {
                    Ok(req) => {
                        match crate::session_launch::spawn_new_session_with_id(
                            &req.workspace_path,
                            &req.prompt,
                            req.model.as_deref(),
                            req.effort.as_deref(),
                            req.permission_mode.as_deref(),
                            req.session_id.as_deref(),
                        ) {
                            Ok(resp) => {
                                let body =
                                    serde_json::to_string(&resp).unwrap_or_default();
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

            crate::routes::SKILL_HISTORY => {
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

            crate::routes::WORKFLOW_TREES => {
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

            crate::routes::TOKEN_BREAKDOWN => {
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

            crate::routes::EXPLORER_ROOTS | crate::routes::EXPLORER_DIR | crate::routes::EXPLORER_FILE | crate::routes::SCRATCHPAD_DIR
            | crate::routes::SCRATCHPAD_FILE => {
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

            crate::routes::GIT_STATUS | crate::routes::GIT_PUSH | crate::routes::GIT_PULL
                if path == crate::routes::GIT_STATUS
                    || request.method() == &tiny_http::Method::Post =>
            {
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

            crate::routes::SKILL_CONTENT => {
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

            crate::routes::SKILL_FILES => {
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

            crate::routes::SKILL_DELETE if request.method() == &tiny_http::Method::Post => {
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

            crate::routes::HOOKS_PLAN => {
                let plan = hooks::plan_hook_setup();
                let body = serde_json::to_string(&plan).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::APPLY_HOOKS => {
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

            crate::routes::SOURCES_CONFIG => {
                let info = agent_source::get_sources_config_local();
                let body = serde_json::to_string(&info).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::SET_SOURCE_ENABLED => {
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

            crate::routes::LIST_CLAUDE_BINARIES => {
                let bins = crate::claude_binary::discover();
                let body = serde_json::to_string(&bins).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::CLAUDE_BINARY_OVERRIDE => {
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

            crate::routes::REMOVE_HOOKS => {
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
            crate::routes::APPLY_GUARD_HOOK => {
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

            crate::routes::REMOVE_GUARD_HOOK => {
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

            crate::routes::GUARD_PENDING => {
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

            crate::routes::GUARD_RESPOND => {
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

            crate::routes::GUARD_ANALYZE => {
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

            crate::routes::GUARD_ALLOW_RULES => {
                let rules = audit::list_guard_allow_rules();
                let body = serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::GUARD_ALLOW_RULES_REMOVE => {
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
            crate::routes::APPLY_ELICITATION_HOOK => {
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

            crate::routes::REMOVE_ELICITATION_HOOK => {
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

            crate::routes::ELICITATION_PENDING => {
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

            // ── Session mark (manual done/pending review state) ──────────────
            crate::routes::SESSION_MARK => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::session_mark::SetSessionMarkRequest>(
                    &body_bytes,
                ) {
                    Ok(req_body) => {
                        match crate::session_mark::set_mark(
                            &req_body.session_id,
                            &req_body.workspace_path,
                            req_body.mark,
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

            // ── Session read (batch mark-read; unread is derived) ─────────────
            crate::routes::SESSION_READ => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::session_read::MarkSessionsReadRequest>(
                    &body_bytes,
                ) {
                    Ok(req_body) => match crate::session_read::mark_read(&req_body.items) {
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

            // ── Workspace command runner (proc_runner) ───────────────────────
            crate::routes::PROCS => {
                let body =
                    serde_json::to_string(&crate::proc_runner::list_procs()).unwrap_or_default();
                let _ = request
                    .respond(tiny_http::Response::from_string(body).with_header(json_header));
            }

            crate::routes::PROC_RUN if request.method() == &tiny_http::Method::Post => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::proc_runner::SpawnProcRequest>(&body_bytes) {
                    Ok(req_body) => {
                        let host_exe = std::env::current_exe()
                            .map_err(|e| format!("cannot locate fleet binary: {e}"));
                        let result = host_exe.and_then(|exe| {
                            crate::proc_runner::spawn_proc(
                                &exe,
                                &req_body.workspace_path,
                                &req_body.command,
                                req_body.cols,
                                req_body.rows,
                            )
                        });
                        match result {
                            Ok(rec) => {
                                let body = serde_json::to_string(&rec).unwrap_or_default();
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

            crate::routes::PROC_OUTPUT => {
                let id = query.get("id").map(|s| s.as_str()).unwrap_or("");
                let offset: Option<u64> = query.get("offset").and_then(|s| s.parse().ok());
                match crate::proc_runner::proc_output(id, offset) {
                    Ok(chunk) => {
                        let body = serde_json::to_string(&chunk).unwrap_or_default();
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

            crate::routes::PROC_INPUT if request.method() == &tiny_http::Method::Post => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::proc_runner::ProcInputRequest>(&body_bytes) {
                    Ok(req_body) => {
                        match crate::proc_runner::proc_input(&req_body.id, &req_body.data_b64) {
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

            crate::routes::PROC_RESIZE if request.method() == &tiny_http::Method::Post => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::proc_runner::ProcResizeRequest>(&body_bytes) {
                    Ok(req_body) => {
                        match crate::proc_runner::proc_resize(
                            &req_body.id,
                            req_body.cols,
                            req_body.rows,
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

            crate::routes::PROC_KILL if request.method() == &tiny_http::Method::Post => {
                let id = query.get("id").map(|s| s.as_str()).unwrap_or("");
                let force = query.get("force").map(|s| s == "true").unwrap_or(false);
                match crate::proc_runner::kill_proc(id, force) {
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

            crate::routes::PROC_CLEAR if request.method() == &tiny_http::Method::Post => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::proc_runner::ClearProcRequest>(&body_bytes) {
                    Ok(req_body) => {
                        let result = match req_body.id {
                            Some(id) => crate::proc_runner::clear_proc(&id).map(|()| 1u32),
                            None => crate::proc_runner::clear_finished_procs(
                                req_body.workspace_path.as_deref(),
                            ),
                        };
                        match result {
                            Ok(cleared) => {
                                let body = serde_json::json!({"cleared": cleared}).to_string();
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

            // ── Interaction mode endpoints ───────────────────────────────────
            crate::routes::APPLY_INTERACTION_MODE => {
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

            crate::routes::REMOVE_INTERACTION_MODE => {
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

            // ── Wiki guidance endpoints ──────────────────────────────────────
            crate::routes::APPLY_WIKI_GUIDANCE => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                #[derive(serde::Deserialize)]
                struct Req { locale: String }
                let locale = serde_json::from_slice::<Req>(&body_bytes)
                    .map(|r| r.locale)
                    .unwrap_or_else(|_| "en".to_string());
                match crate::wiki_guidance::apply_wiki_guidance(&locale) {
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

            crate::routes::REMOVE_WIKI_GUIDANCE => {
                match crate::wiki_guidance::remove_wiki_guidance() {
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

            crate::routes::INTERACTION_DIAGNOSTICS => {
                let checks = crate::interaction_mode_diagnostics::run_checks();
                let body = serde_json::to_string(&checks).unwrap_or_else(|_| "[]".into());
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::TEST_DECISION_END_TO_END => {
                match crate::interaction_mode_test::run_end_to_end_test(
                    std::time::Duration::from_secs(10),
                ) {
                    Ok(r) => {
                        let body = serde_json::to_string(&r).unwrap_or_default();
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

            crate::routes::TEST_DECISION_VIA_CLAUDE_CLI => {
                match crate::interaction_mode_test::run_claude_cli_test(
                    std::time::Duration::from_secs(60),
                ) {
                    Ok(r) => {
                        let body = serde_json::to_string(&r).unwrap_or_default();
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

            // ── PRD Discipline mode endpoints ────────────────────────────────
            crate::routes::APPLY_PRD_MODE => {
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

            crate::routes::REMOVE_PRD_MODE => {
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
            crate::routes::APPLY_PLAN_APPROVAL_HOOK => {
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

            crate::routes::REMOVE_PLAN_APPROVAL_HOOK => {
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

            crate::routes::PLAN_APPROVAL_PENDING => {
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
                // Cards whose wait timed out live in the parked store instead of the
                // channel's request dir — the producer that was blocking on them is
                // gone. They stay pending here until the user resolves them.
                for mut req in crate::parked::list_requests::<plan_approval::PlanApprovalRequest>(crate::parked::ParkedKind::PlanApproval) {
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

            crate::routes::PLAN_APPROVAL_RESPOND => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<plan_approval::PlanApprovalResponse>(&body_bytes) {
                    Ok(resp) => {
                        // A parked card has no producer left polling for a response
                        // file, so `deliver` resumes the session with the answer
                        // instead (or drops the card when the user dismissed it).
                        let outcome =
                            crate::parked::deliver(&resp.id, &resp, false, plan_approval::write_response);
                        match outcome {
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

            crate::routes::ELICITATION_RESPOND => {
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

            crate::routes::FLEET_ASK_PENDING => {
                let ids = crate::mcp_ipc::list_pending_requests();
                let sessions = scan_all_sources(&sources);
                let mut requests = Vec::new();
                for id in &ids {
                    if let Some(mut req) = crate::mcp_ipc::read_request(id) {
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
                for mut req in crate::parked::list_requests::<crate::mcp_ipc::FleetAskRequest>(crate::parked::ParkedKind::FleetAsk) {
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

            crate::routes::FLEET_ASK_RESPOND => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::mcp_ipc::FleetAskResponse>(&body_bytes) {
                    Ok(resp) => {
                        // A parked card has no producer left polling for a response
                        // file, so `deliver` resumes the session with the answer
                        // instead (or drops the card when the user dismissed it).
                        let outcome =
                            crate::parked::deliver(&resp.id, &resp, resp.cancelled, crate::mcp_ipc::write_response);
                        match outcome {
                            Ok(()) => {
                                // Don't cleanup here — the `fleet mcp` server
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

            crate::routes::PERMISSION_PROMPT_PENDING => {
                let ids = crate::permission_prompt_ipc::list_pending_requests();
                let sessions = scan_all_sources(&sources);
                let mut requests = Vec::new();
                for id in &ids {
                    if let Some(mut req) = crate::permission_prompt_ipc::read_request(id) {
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

            crate::routes::PERMISSION_PROMPT_RESPOND => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::permission_prompt_ipc::PermissionPromptResponse>(&body_bytes) {
                    Ok(resp) => {
                        match crate::permission_prompt_ipc::write_response(&resp) {
                            Ok(()) => {
                                // Don't cleanup here — the `fleet mcp` server
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

            crate::routes::A2UI_RENDER_PENDING => {
                let ids = crate::mcp_a2ui_ipc::list_pending_requests();
                let sessions = scan_all_sources(&sources);
                let mut requests = Vec::new();
                for id in &ids {
                    if let Some(mut req) = crate::mcp_a2ui_ipc::read_request(id) {
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
                for mut req in crate::parked::list_requests::<crate::mcp_a2ui_ipc::A2uiRenderRequest>(crate::parked::ParkedKind::A2uiRender) {
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

            crate::routes::A2UI_RENDER_RESPOND => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<crate::mcp_a2ui_ipc::A2uiRenderResponse>(&body_bytes) {
                    Ok(resp) => {
                        // A parked card has no producer left polling for a response
                        // file, so `deliver` resumes the session with the answer
                        // instead (or drops the card when the user dismissed it).
                        let outcome =
                            crate::parked::deliver(&resp.id, &resp, resp.cancelled, crate::mcp_a2ui_ipc::write_response);
                        match outcome {
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

            crate::routes::ELICITATION_UPLOAD => {
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
                            continue;
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
                        continue;
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
                        continue;
                    }
                    dest
                };

                let abs = dest.to_string_lossy().into_owned();
                let body = serde_json::json!({"path": abs}).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::USER_ATTACHMENT => {
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

            crate::routes::SEARCH => {
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

            crate::routes::AUDIT => {
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

            crate::routes::AUDIT_PATTERN_INFO => {
                let (version, path) = crate::pattern_update::get_patterns_info();
                let body = serde_json::json!({
                    "version": version,
                    "path": path,
                }).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::AUDIT_CHECK_UPDATE => {
                let msg = crate::pattern_update::check_update_now();
                let body = serde_json::json!({ "message": msg }).to_string();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::AUDIT_RULES => {
                let rules = crate::audit::get_all_rules();
                let body = serde_json::to_string(&rules).unwrap_or_else(|_| "[]".into());
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::AUDIT_RULES_TOGGLE => {
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

            crate::routes::AUDIT_RULES_SAVE => {
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

            crate::routes::AUDIT_RULES_DELETE => {
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

            crate::routes::AUDIT_RULES_SUGGEST => {
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

            crate::routes::DAILY_REPORT => {
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

            crate::routes::DAILY_REPORT_STATS => {
                let from = query.get("from").cloned().unwrap_or_default();
                let to = query.get("to").cloned().unwrap_or_default();
                let store = report_store.lock().unwrap();
                let stats = store.list_stats(&from, &to).unwrap_or_default();
                let body = serde_json::to_string(&stats).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::DAILY_REPORT_GENERATE => {
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

            crate::routes::DAILY_REPORT_AI_SUMMARY => {
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

            crate::routes::DAILY_REPORT_LESSONS => {
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

            crate::routes::DAILY_REPORT_APPEND_LESSON => {
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
            crate::routes::LLM_PROVIDERS => {
                let infos = llm_provider::all_provider_infos();
                let body = serde_json::to_string(&infos).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::LLM_CONFIG if request.method() == &tiny_http::Method::Get => {
                let cfg = llm_config.lock().unwrap().clone();
                let body = serde_json::to_string(&cfg).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::LLM_CONFIG => {
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

            crate::routes::FLEET_LLM_USAGE_DAILY => {
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

            crate::routes::USAGE_HISTORY => {
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

            // ── Session outcome analysis (delegated from remote clients) ──
            crate::routes::ANALYZE => {
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

            crate::routes::MOBILE_RELAY_CONFIG if request.method() == &tiny_http::Method::Get => {
                let cfg = crate::mobile_relay::load_config();
                let body = serde_json::to_string(&cfg).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::MOBILE_RELAY_CONFIG if request.method() == &tiny_http::Method::Post => {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                let parsed: Result<crate::mobile_relay::MobileRelayConfig, _> =
                    serde_json::from_slice(&body_bytes);
                let (status, body) = match parsed {
                    Ok(cfg) => match crate::mobile_relay::set_config_normalized(cfg) {
                        Ok(stored) => (200, serde_json::to_string(&stored).unwrap_or_default()),
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

            crate::routes::MOBILE_RELAY_ROTATE if request.method() == &tiny_http::Method::Post => {
                let (status, body) = match crate::mobile_relay::rotate_secret() {
                    Ok(cfg) => (200, serde_json::to_string(&cfg).unwrap_or_default()),
                    Err(e) => (500, serde_json::json!({"error": e}).to_string()),
                };
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(status)
                        .with_header(json_header),
                );
            }

            crate::routes::MOBILE_RELAY_STATUS => {
                let status = crate::mobile_relay::status();
                let body = serde_json::to_string(&status).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

            crate::routes::MOBILE_RELAY_QR => {
                let (status, body) = match crate::mobile_relay::qr_svg() {
                    Ok(svg) => (200, serde_json::json!({"svg": svg}).to_string()),
                    Err(e) => (404, serde_json::json!({"error": e}).to_string()),
                };
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
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
