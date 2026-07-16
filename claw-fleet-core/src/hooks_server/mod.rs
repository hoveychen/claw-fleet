//! Shared lib backing the `fleet serve` and `fleet-hooks-server` binaries.
//!
//! Phase 4 P1 extracted the 3000-line `cmd_serve` body that used to live in
//! `fleet-cli/src/main.rs` into this module so both binaries can call the
//! same `serve()` function instead of trampolining through `execvp`.

pub mod sse;

pub use sse::{handle_sse_upgrade, SseBroadcaster, SseClient};

mod routes_audit;
mod routes_core;
mod routes_daily_report;
mod routes_decision;
mod routes_elicitation;
mod routes_explorer;
mod routes_guard;
mod routes_hooks;
mod routes_interaction;
mod routes_llm;
mod routes_memory;
mod routes_mobile;
mod routes_plan_approval;
mod routes_proc;
mod routes_session_state;
mod routes_skills;
mod routes_spawn;
mod routes_wiki;
use routes_audit::*;
use routes_core::*;
use routes_daily_report::*;
use routes_decision::*;
use routes_elicitation::*;
use routes_explorer::*;
use routes_guard::*;
use routes_hooks::*;
use routes_interaction::*;
use routes_llm::*;
use routes_memory::*;
use routes_mobile::*;
use routes_plan_approval::*;
use routes_proc::*;
use routes_session_state::*;
use routes_skills::*;
use routes_spawn::*;
use routes_wiki::*;

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

/// Shared, long-lived serve() state threaded into every route handler.
/// All fields are read-only references (the Arc<Mutex<_>> ones are locked
/// internally); handlers rebind what they need from here.
pub(crate) struct ServeCtx<'a> {
    pub(crate) sources: &'a [Box<dyn crate::agent_source::AgentSource>],
    pub(crate) report_store: &'a std::sync::Arc<std::sync::Mutex<crate::daily_report::ReportStore>>,
    pub(crate) llm_config: &'a std::sync::Arc<std::sync::Mutex<crate::llm_provider::LlmConfig>>,
    pub(crate) audit_history: &'a std::sync::Arc<std::sync::Mutex<crate::audit::AuditHistory>>,
    pub(crate) search_index: &'a crate::search_index::SearchIndex,
}

pub fn serve(port: u16, token: String, port_file: Option<std::path::PathBuf>) {

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
    // Codex parallel sampler — self-gates on codex being installed each tick.
    crate::codex_source::start_codex_background_sampler(std::time::Duration::from_secs(600));

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

    let ctx = &ServeCtx {
        sources: &sources,
        report_store: &report_store,
        llm_config: &llm_config,
        audit_history: &audit_history,
        search_index: &search_index,
    };

    for request in server.incoming_requests() {
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
            crate::routes::HEALTH => route_health(ctx, request, &query, json_header, path),

            crate::routes::SESSIONS => route_sessions(ctx, request, &query, json_header, path),

            crate::routes::INTERRUPT => route_interrupt(ctx, request, &query, json_header, path),

            crate::routes::STOP => route_stop(ctx, request, &query, json_header, path),

            // `/stop_workspace` kills every agent process (and its tree) rooted
            // in a workspace. Retired in Phase 4 as a "lifecycle endpoint", but
            // RemoteBackend::kill_workspace never stopped calling it, so a remote
            // stop on an imprecise pid 404'd silently. Re-added to match the
            // still-live client. `?path=` is percent-encoded (slashes as %2F).
            crate::routes::STOP_WORKSPACE => route_stop_workspace(ctx, request, &query, json_header, path),

            // Phase 4 P3: `/auto_resume_config` removed. Remote callers hitting
            // that path receive the catch-all 404 below. `/resume_session` was
            // re-added later (POST, with follow-up prompt) for the history
            // panel — see the handler further down.

            // ── Unified /sources/{name}/account and /sources/{name}/usage ──
            _ if path.starts_with(crate::routes::SOURCES_PREFIX) => route_sources_prefix(ctx, request, &query, json_header, path),

            crate::routes::SETUP_STATUS => route_setup_status(ctx, request, &query, json_header, path),

            crate::routes::USAGE_SUMMARIES => route_usage_summaries(ctx, request, &query, json_header, path),

            crate::routes::TODAY_USAGE => route_today_usage(ctx, request, &query, json_header, path),

            crate::routes::SESSION_DECISIONS => route_session_decisions(ctx, request, &query, json_header, path),

            crate::routes::MESSAGES => route_messages(ctx, request, &query, json_header, path),

            crate::routes::TOOL_RESULT => route_tool_result(ctx, request, &query, json_header, path),

            crate::routes::FILE_SIZE => route_file_size(ctx, request, &query, json_header, path),

            crate::routes::TAIL => route_tail(ctx, request, &query, json_header, path),

            crate::routes::MEMORIES => route_memories(ctx, request, &query, json_header, path),

            crate::routes::LIVE_THINKING => route_live_thinking(ctx, request, &query, json_header, path),

            crate::routes::MEMORY_CONTENT => route_memory_content(ctx, request, &query, json_header, path),

            crate::routes::MEMORY_HISTORY => route_memory_history(ctx, request, &query, json_header, path),

            crate::routes::HANDOFF_CHAIN => route_handoff_chain(ctx, request, &query, json_header, path),

            crate::routes::WIKI_DOCS => route_wiki_docs(ctx, request, &query, json_header, path),

            crate::routes::WIKI_DOC => route_wiki_doc(ctx, request, &query, json_header, path),

            crate::routes::WIKI_FILE => route_wiki_file(ctx, request, &query, json_header, path),

            crate::routes::DECISION_ASSET => route_decision_asset(ctx, request, &query, json_header, path),

            crate::routes::WIKI_SEARCH => route_wiki_search(ctx, request, &query, json_header, path),

            crate::routes::WIKI_EXPORT => route_wiki_export(ctx, request, &query, json_header, path),

            crate::routes::WIKI_DELETE if request.method() == &tiny_http::Method::Post => route_wiki_delete(ctx, request, &query, json_header, path),

            crate::routes::WIKI_MOVE if request.method() == &tiny_http::Method::Post => route_wiki_move(ctx, request, &query, json_header, path),

            // Folder ops. `to: ""` dissolves the folder into the tree root.
            crate::routes::WIKI_MOVE_FOLDER if request.method() == &tiny_http::Method::Post => route_wiki_move_folder(ctx, request, &query, json_header, path),

            crate::routes::WIKI_DELETE_FOLDER if request.method() == &tiny_http::Method::Post => route_wiki_delete_folder(ctx, request, &query, json_header, path),

            crate::routes::TASK_PLANS => route_task_plans(ctx, request, &query, json_header, path),

            crate::routes::SKILLS => route_skills(ctx, request, &query, json_header, path),

            crate::routes::PLUGINS => route_plugins(ctx, request, &query, json_header, path),

            crate::routes::PLUGINS_SET_ENABLED
                if request.method() == &tiny_http::Method::Post => route_plugins_set_enabled(ctx, request, &query, json_header, path),

            crate::routes::PLUGINS_MARKETPLACES if request.method() == &tiny_http::Method::Get => route_plugins_marketplaces(ctx, request, &query, json_header, path),

            crate::routes::PLUGINS_MARKETPLACES_ADD if request.method() == &tiny_http::Method::Post => route_plugins_marketplaces_add(ctx, request, &query, json_header, path),

            crate::routes::PLUGINS_MARKETPLACES_REMOVE if request.method() == &tiny_http::Method::Post => route_plugins_marketplaces_remove(ctx, request, &query, json_header, path),

            crate::routes::PLUGINS_INSTALL | crate::routes::PLUGINS_UNINSTALL
                if request.method() == &tiny_http::Method::Post => route_plugins_install(ctx, request, &query, json_header, path),

            // Resume a scanned session with an optional follow-up prompt
            // (history panel's "恢复会话", remote backend). Detached
            // `claude --resume <sid> -p <prompt>`; the resumed turn appears
            // via the scanner as the JSONL grows.
            crate::routes::RESUME_SESSION if request.method() == &tiny_http::Method::Post => route_resume_session(ctx, request, &query, json_header, path),

            // Queue a follow-up message for a session still mid-turn; the
            // desktop tick delivers it via `claude --resume` when the turn ends.
            crate::routes::ENQUEUE_MESSAGE if request.method() == &tiny_http::Method::Post => route_enqueue_message(ctx, request, &query, json_header, path),

            // Absolute path of this host's pure-chat workspace, created on
            // demand. The remote desktop pins it in its launcher but cannot
            // derive it — the path is under *this* host's home.
            crate::routes::CHAT_WORKSPACE if request.method() == &tiny_http::Method::Get => route_chat_workspace(ctx, request, &query, json_header, path),

            // Directory picker for the remote desktop's launcher. Same reason
            // /chat_workspace exists: the directory the user is choosing lives on
            // *this* host, and the desktop's native OS dialog can only browse its
            // own disk. `path` absent = this host's home.
            //
            // Not gated on known session workspaces — picking a directory that has
            // never had a session is the entire point. `workspace_browse` carries
            // its own boundary (home + off-home known workspaces, canonical).
            crate::routes::BROWSE_DIR if request.method() == &tiny_http::Method::Get => route_browse_dir(ctx, request, &query, json_header, path),

            // Spawn a brand-new headless Claude Code session (sessions page's
            // "new session" button, remote backend). Detached `claude -p`;
            // the session appears via the scanner once its JSONL exists.
            crate::routes::SPAWN_SESSION if request.method() == &tiny_http::Method::Post => route_spawn_session(ctx, request, &query, json_header, path),

            crate::routes::SKILL_HISTORY => route_skill_history(ctx, request, &query, json_header, path),

            crate::routes::WORKFLOW_TREES => route_workflow_trees(ctx, request, &query, json_header, path),

            crate::routes::TOKEN_BREAKDOWN => route_token_breakdown(ctx, request, &query, json_header, path),

            crate::routes::CODEX_TOKEN_BREAKDOWN => route_codex_token_breakdown(ctx, request, &query, json_header, path),

            crate::routes::EXPLORER_ROOTS | crate::routes::EXPLORER_DIR | crate::routes::EXPLORER_FILE | crate::routes::SCRATCHPAD_DIR
            | crate::routes::SCRATCHPAD_FILE => route_explorer_roots(ctx, request, &query, json_header, path),

            crate::routes::GIT_STATUS | crate::routes::GIT_PUSH | crate::routes::GIT_PULL
                if path == crate::routes::GIT_STATUS
                    || request.method() == &tiny_http::Method::Post => route_git_status(ctx, request, &query, json_header, path),

            crate::routes::SKILL_CONTENT => route_skill_content(ctx, request, &query, json_header, path),

            crate::routes::SKILL_FILES => route_skill_files(ctx, request, &query, json_header, path),

            crate::routes::SKILL_DELETE if request.method() == &tiny_http::Method::Post => route_skill_delete(ctx, request, &query, json_header, path),

            crate::routes::HOOKS_PLAN => route_hooks_plan(ctx, request, &query, json_header, path),

            crate::routes::APPLY_HOOKS => route_apply_hooks(ctx, request, &query, json_header, path),

            crate::routes::SOURCES_CONFIG => route_sources_config(ctx, request, &query, json_header, path),

            crate::routes::SET_SOURCE_ENABLED => route_set_source_enabled(ctx, request, &query, json_header, path),

            crate::routes::LIST_CLAUDE_BINARIES => route_list_claude_binaries(ctx, request, &query, json_header, path),

            crate::routes::CLAUDE_BINARY_OVERRIDE => route_claude_binary_override(ctx, request, &query, json_header, path),

            crate::routes::REMOVE_HOOKS => route_remove_hooks(ctx, request, &query, json_header, path),

            // ── Guard hook endpoints ──────────────────────────────────────
            crate::routes::APPLY_GUARD_HOOK => route_apply_guard_hook(ctx, request, &query, json_header, path),

            crate::routes::REMOVE_GUARD_HOOK => route_remove_guard_hook(ctx, request, &query, json_header, path),

            crate::routes::GUARD_PENDING => route_guard_pending(ctx, request, &query, json_header, path),

            crate::routes::GUARD_RESPOND => route_guard_respond(ctx, request, &query, json_header, path),

            crate::routes::GUARD_ANALYZE => route_guard_analyze(ctx, request, &query, json_header, path),

            crate::routes::GUARD_ALLOW_RULES => route_guard_allow_rules(ctx, request, &query, json_header, path),

            crate::routes::GUARD_ALLOW_RULES_REMOVE => route_guard_allow_rules_remove(ctx, request, &query, json_header, path),

            // ── Elicitation hook endpoints ───────────────────────────────────
            crate::routes::APPLY_ELICITATION_HOOK => route_apply_elicitation_hook(ctx, request, &query, json_header, path),

            crate::routes::REMOVE_ELICITATION_HOOK => route_remove_elicitation_hook(ctx, request, &query, json_header, path),

            crate::routes::ELICITATION_PENDING => route_elicitation_pending(ctx, request, &query, json_header, path),

            // ── Session mark (manual done/pending review state) ──────────────
            crate::routes::SESSION_MARK => route_session_mark(ctx, request, &query, json_header, path),

            // ── Session read (batch mark-read; unread is derived) ─────────────
            crate::routes::SESSION_READ => route_session_read(ctx, request, &query, json_header, path),

            // ── Workspace command runner (proc_runner) ───────────────────────
            crate::routes::PROCS => route_procs(ctx, request, &query, json_header, path),

            crate::routes::PROC_RUN if request.method() == &tiny_http::Method::Post => route_proc_run(ctx, request, &query, json_header, path),

            crate::routes::PROC_OUTPUT => route_proc_output(ctx, request, &query, json_header, path),

            crate::routes::PROC_INPUT if request.method() == &tiny_http::Method::Post => route_proc_input(ctx, request, &query, json_header, path),

            crate::routes::PROC_RESIZE if request.method() == &tiny_http::Method::Post => route_proc_resize(ctx, request, &query, json_header, path),

            crate::routes::PROC_KILL if request.method() == &tiny_http::Method::Post => route_proc_kill(ctx, request, &query, json_header, path),

            crate::routes::PROC_CLEAR if request.method() == &tiny_http::Method::Post => route_proc_clear(ctx, request, &query, json_header, path),

            // ── Interaction mode endpoints ───────────────────────────────────
            crate::routes::APPLY_INTERACTION_MODE => route_apply_interaction_mode(ctx, request, &query, json_header, path),

            crate::routes::REMOVE_INTERACTION_MODE => route_remove_interaction_mode(ctx, request, &query, json_header, path),

            // ── Wiki guidance endpoints ──────────────────────────────────────
            crate::routes::APPLY_WIKI_GUIDANCE => route_apply_wiki_guidance(ctx, request, &query, json_header, path),

            crate::routes::REMOVE_WIKI_GUIDANCE => route_remove_wiki_guidance(ctx, request, &query, json_header, path),

            // ── Model guidance endpoints ─────────────────────────────────────
            crate::routes::APPLY_MODEL_GUIDANCE => route_apply_model_guidance(ctx, request, &query, json_header, path),

            crate::routes::REMOVE_MODEL_GUIDANCE => route_remove_model_guidance(ctx, request, &query, json_header, path),

            crate::routes::INTERACTION_DIAGNOSTICS => route_interaction_diagnostics(ctx, request, &query, json_header, path),

            crate::routes::TEST_DECISION_END_TO_END => route_test_decision_end_to_end(ctx, request, &query, json_header, path),

            crate::routes::TEST_DECISION_VIA_CLAUDE_CLI => route_test_decision_via_claude_cli(ctx, request, &query, json_header, path),

            // ── PRD Discipline mode endpoints ────────────────────────────────
            crate::routes::APPLY_PRD_MODE => route_apply_prd_mode(ctx, request, &query, json_header, path),

            crate::routes::REMOVE_PRD_MODE => route_remove_prd_mode(ctx, request, &query, json_header, path),

            // ── Codex guidance endpoint (writes remote ~/.codex/AGENTS.md) ────
            crate::routes::RECONCILE_CODEX_GUIDANCE => route_reconcile_codex_guidance(ctx, request, &query, json_header, path),

            // ── Plan-approval hook endpoints ─────────────────────────────────
            crate::routes::APPLY_PLAN_APPROVAL_HOOK => route_apply_plan_approval_hook(ctx, request, &query, json_header, path),

            crate::routes::REMOVE_PLAN_APPROVAL_HOOK => route_remove_plan_approval_hook(ctx, request, &query, json_header, path),

            crate::routes::PLAN_APPROVAL_PENDING => route_plan_approval_pending(ctx, request, &query, json_header, path),

            crate::routes::PLAN_APPROVAL_RESPOND => route_plan_approval_respond(ctx, request, &query, json_header, path),

            crate::routes::ELICITATION_RESPOND => route_elicitation_respond(ctx, request, &query, json_header, path),

            crate::routes::FLEET_ASK_PENDING => route_fleet_ask_pending(ctx, request, &query, json_header, path),

            crate::routes::FLEET_ASK_RESPOND => route_fleet_ask_respond(ctx, request, &query, json_header, path),

            crate::routes::PERMISSION_PROMPT_PENDING => route_permission_prompt_pending(ctx, request, &query, json_header, path),

            crate::routes::PERMISSION_PROMPT_RESPOND => route_permission_prompt_respond(ctx, request, &query, json_header, path),

            crate::routes::A2UI_RENDER_PENDING => route_a2ui_render_pending(ctx, request, &query, json_header, path),

            crate::routes::A2UI_RENDER_RESPOND => route_a2ui_render_respond(ctx, request, &query, json_header, path),

            crate::routes::ELICITATION_UPLOAD => route_elicitation_upload(ctx, request, &query, json_header, path),

            crate::routes::USER_ATTACHMENT => route_user_attachment(ctx, request, &query, json_header, path),

            crate::routes::SEARCH => route_search(ctx, request, &query, json_header, path),

            crate::routes::AUDIT => route_audit(ctx, request, &query, json_header, path),

            crate::routes::AUDIT_PATTERN_INFO => route_audit_pattern_info(ctx, request, &query, json_header, path),

            crate::routes::AUDIT_CHECK_UPDATE => route_audit_check_update(ctx, request, &query, json_header, path),

            crate::routes::AUDIT_RULES => route_audit_rules(ctx, request, &query, json_header, path),

            crate::routes::AUDIT_RULES_TOGGLE => route_audit_rules_toggle(ctx, request, &query, json_header, path),

            crate::routes::AUDIT_RULES_SAVE => route_audit_rules_save(ctx, request, &query, json_header, path),

            crate::routes::AUDIT_RULES_DELETE => route_audit_rules_delete(ctx, request, &query, json_header, path),

            crate::routes::AUDIT_RULES_SUGGEST => route_audit_rules_suggest(ctx, request, &query, json_header, path),

            crate::routes::DAILY_REPORT => route_daily_report(ctx, request, &query, json_header, path),

            crate::routes::DAILY_REPORT_STATS => route_daily_report_stats(ctx, request, &query, json_header, path),

            crate::routes::DAILY_REPORT_GENERATE => route_daily_report_generate(ctx, request, &query, json_header, path),

            crate::routes::DAILY_REPORT_AI_SUMMARY => route_daily_report_ai_summary(ctx, request, &query, json_header, path),

            crate::routes::DAILY_REPORT_LESSONS => route_daily_report_lessons(ctx, request, &query, json_header, path),

            crate::routes::DAILY_REPORT_APPEND_LESSON => route_daily_report_append_lesson(ctx, request, &query, json_header, path),

            // ── LLM provider endpoints ──────────────────────────────────
            crate::routes::LLM_PROVIDERS => route_llm_providers(ctx, request, &query, json_header, path),

            crate::routes::LLM_CONFIG if request.method() == &tiny_http::Method::Get => route_llm_config_get(ctx, request, &query, json_header, path),

            crate::routes::LLM_CONFIG => route_llm_config_1(ctx, request, &query, json_header, path),

            crate::routes::FLEET_LLM_USAGE_DAILY => route_fleet_llm_usage_daily(ctx, request, &query, json_header, path),

            crate::routes::USAGE_HISTORY => route_usage_history(ctx, request, &query, json_header, path),
            crate::routes::CODEX_USAGE_HISTORY => route_codex_usage_history(ctx, request, &query, json_header, path),

            // ── Session outcome analysis (delegated from remote clients) ──
            crate::routes::ANALYZE => route_analyze(ctx, request, &query, json_header, path),

            crate::routes::MOBILE_RELAY_CONFIG if request.method() == &tiny_http::Method::Get => route_mobile_relay_config_get(ctx, request, &query, json_header, path),

            crate::routes::MOBILE_RELAY_CONFIG if request.method() == &tiny_http::Method::Post => route_mobile_relay_config_post(ctx, request, &query, json_header, path),

            crate::routes::MOBILE_RELAY_ROTATE if request.method() == &tiny_http::Method::Post => route_mobile_relay_rotate(ctx, request, &query, json_header, path),

            crate::routes::MOBILE_RELAY_STATUS => route_mobile_relay_status(ctx, request, &query, json_header, path),

            crate::routes::MOBILE_RELAY_QR => route_mobile_relay_qr(ctx, request, &query, json_header, path),

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
