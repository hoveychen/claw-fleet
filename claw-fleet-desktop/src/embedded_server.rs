//! Embedded HTTP server for mobile access.
//!
//! Runs inside the Tauri desktop app, sharing the same `Backend` instance
//! (via `AppState`).  Endpoints mirror the `fleet serve` API so that mobile
//! clients (and any HTTP client) can consume the same data.
//!
//! The server also exposes a Server-Sent Events (SSE) endpoint at `/events`
//! for real-time push of session updates and alerts.

use std::collections::HashMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};

use percent_encoding::percent_decode_str;
use serde::Serialize;

use crate::backend::Backend;
use crate::daily_report::Lesson;
use crate::llm_provider::LlmConfig;

// ── SSE client management ───────────────────────────────────────────────────

/// A connected SSE client (kept alive for streaming events).
struct SseClient {
    stream: Box<dyn std::io::Write + Send>,
    alive: bool,
}

impl SseClient {
    fn send_event(&mut self, event_type: &str, data: &str) {
        let msg = format!("event: {event_type}\ndata: {data}\n\n");
        if self.stream.write_all(msg.as_bytes()).is_err() {
            self.alive = false;
        }
        let _ = self.stream.flush();
    }
}

/// Thread-safe collection of SSE clients.
#[derive(Clone)]
pub struct SseBroadcaster {
    clients: Arc<Mutex<Vec<SseClient>>>,
}

impl SseBroadcaster {
    fn new() -> Self {
        Self {
            clients: Arc::new(Mutex::new(Vec::new())),
        }
    }

    fn add_client(&self, stream: Box<dyn std::io::Write + Send>) {
        self.clients.lock().unwrap().push(SseClient {
            stream,
            alive: true,
        });
    }

    /// Broadcast an event to all connected SSE clients.
    pub fn broadcast(&self, event_type: &str, data: &str) {
        let mut clients = self.clients.lock().unwrap();
        for client in clients.iter_mut() {
            client.send_event(event_type, data);
        }
        clients.retain(|c| c.alive);
    }

    /// Number of connected clients.
    pub fn client_count(&self) -> usize {
        self.clients.lock().unwrap().len()
    }
}

// ── Embedded server ─────────────────────────────────────────────────────────

pub struct EmbeddedServer {
    port: u16,
    token: String,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    sse: SseBroadcaster,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MobileServerStatus {
    pub running: bool,
    pub port: u16,
    pub token: String,
    pub tunnel_url: Option<String>,
    pub connected_clients: usize,
}

impl EmbeddedServer {
    /// Start the embedded HTTP server on the given port, sharing the Backend.
    /// Blocks until the server is confirmed listening (or returns an error).
    pub fn start(
        backend: Arc<RwLock<Box<dyn Backend>>>,
        port: u16,
        token: String,
    ) -> Result<Self, String> {
        let shutdown = Arc::new(AtomicBool::new(false));
        let sse = SseBroadcaster::new();

        // Channel to confirm the server bound successfully.
        let (bind_tx, bind_rx) = std::sync::mpsc::channel();

        let thread = {
            let shutdown = shutdown.clone();
            let token = token.clone();
            let sse = sse.clone();

            thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_server(backend, port, &token, shutdown, sse, bind_tx);
                }));
                if let Err(e) = result {
                    let msg = if let Some(s) = e.downcast_ref::<&str>() {
                        s.to_string()
                    } else if let Some(s) = e.downcast_ref::<String>() {
                        s.clone()
                    } else {
                        "unknown panic".to_string()
                    };
                    eprintln!("[embedded-server] PANIC: {msg}");
                }
            })
        };

        // Wait for the server to confirm it's listening.
        match bind_rx.recv_timeout(std::time::Duration::from_secs(5)) {
            Ok(Ok(())) => {}
            Ok(Err(e)) => return Err(format!("server bind failed on port {port}: {e}")),
            Err(_) => return Err(format!("server startup timed out on port {port}")),
        }

        Ok(EmbeddedServer {
            port,
            token,
            shutdown,
            thread: Some(thread),
            sse,
        })
    }

    pub fn stop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
        // Connect to the server to unblock the accept loop.
        let _ = std::net::TcpStream::connect_timeout(
            &format!("127.0.0.1:{}", self.port).parse().unwrap(),
            std::time::Duration::from_secs(1),
        );
        if let Some(t) = self.thread.take() {
            // Don't block forever — give it 3 seconds then detach.
            let (tx, rx) = std::sync::mpsc::channel();
            let join_thread = thread::spawn(move || {
                let _ = t.join();
                let _ = tx.send(());
            });
            if rx.recv_timeout(std::time::Duration::from_secs(3)).is_err() {
                eprintln!("[embedded-server] stop timed out, detaching thread");
                // Let the join_thread run in background; it will eventually finish
                // when the server loop hits the shutdown flag.
                drop(join_thread);
            }
        }
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn token(&self) -> &str {
        &self.token
    }

    pub fn broadcaster(&self) -> &SseBroadcaster {
        &self.sse
    }

    pub fn status(&self, tunnel_url: Option<String>) -> MobileServerStatus {
        MobileServerStatus {
            running: !self.shutdown.load(Ordering::Relaxed),
            port: self.port,
            token: self.token.clone(),
            tunnel_url,
            connected_clients: self.sse.client_count(),
        }
    }
}

impl Drop for EmbeddedServer {
    fn drop(&mut self) {
        self.stop();
    }
}

// ── Server loop ─────────────────────────────────────────────────────────────

fn run_server(
    backend: Arc<RwLock<Box<dyn Backend>>>,
    port: u16,
    token: &str,
    shutdown: Arc<AtomicBool>,
    sse: SseBroadcaster,
    bind_result: std::sync::mpsc::Sender<Result<(), String>>,
) {
    let addr = format!("127.0.0.1:{}", port);
    let server = match tiny_http::Server::http(&addr) {
        Ok(s) => s,
        Err(e) => {
            let msg = format!("cannot bind to {addr}: {e}");
            eprintln!("[embedded-server] {msg}");
            let _ = bind_result.send(Err(msg));
            return;
        }
    };
    eprintln!(
        "[embedded-server] listening on {addr} (version {})",
        env!("CARGO_PKG_VERSION")
    );
    let _ = bind_result.send(Ok(()));

    let expected_auth = format!("Bearer {}", token);

    for mut request in server.incoming_requests() {
        if shutdown.load(Ordering::Relaxed) {
            let _ = request.respond(tiny_http::Response::empty(503));
            break;
        }

        let url = request.url().to_string();
        let (path, query_str) = match url.split_once('?') {
            Some((p, q)) => (p, q),
            None => (url.as_str(), ""),
        };
        let query = parse_query_full(query_str);

        // ── Public endpoint: landing page (no auth required) ────────
        if path == "/mobile" {
            let query_token = query.get("token").map(|s| s.as_str()).unwrap_or("");
            let html = build_landing_page(query_token);
            let html_header: tiny_http::Header =
                "Content-Type: text/html; charset=utf-8".parse().unwrap();
            let _ = request.respond(
                tiny_http::Response::from_string(html).with_header(html_header),
            );
            continue;
        }

        // ── Auth check ──────────────────────────────────────────────
        let auth_ok = request.headers().iter().any(|h| {
            h.field.equiv("authorization") && h.value.as_str() == expected_auth.as_str()
        });

        if !auth_ok {
            // Also allow token as query param for SSE (EventSource can't set headers)
            if query.get("token").map(|t| t.as_str()) != Some(token) {
                let _ = request.respond(tiny_http::Response::empty(401));
                continue;
            }
        }

        // Strip the "token" key from query so API handlers don't see it.
        let query: HashMap<String, String> = query
            .into_iter()
            .filter(|(k, _)| k != "token")
            .collect();

        // Handle SSE endpoint specially — it takes over the connection.
        if path == "/events" {
            handle_sse(request, &sse);
            continue;
        }

        let json_header: tiny_http::Header = "Content-Type: application/json".parse().unwrap();
        // CORS headers for mobile web/capacitor
        let cors_origin: tiny_http::Header = "Access-Control-Allow-Origin: *".parse().unwrap();
        let cors_headers: tiny_http::Header =
            "Access-Control-Allow-Headers: Authorization, Content-Type"
                .parse()
                .unwrap();

        // Handle CORS preflight
        if request.method().as_str() == "OPTIONS" {
            let _ = request.respond(
                tiny_http::Response::empty(204)
                    .with_header(cors_origin.clone())
                    .with_header(cors_headers.clone()),
            );
            continue;
        }

        let backend = match backend.read() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("[embedded-server] backend lock poisoned: {e}");
                let _ = request.respond(tiny_http::Response::empty(500));
                continue;
            }
        };

        let response = handle_api_request(
            path,
            &query,
            &mut request,
            &**backend,
        );

        let _ = request.respond(
            response
                .with_header(json_header)
                .with_header(cors_origin)
                .with_header(cors_headers),
        );
    }

    eprintln!("[embedded-server] shutting down");
}

// ── API request handler ────────────────────────────────────────────────────

fn handle_api_request(
    path: &str,
    query: &HashMap<String, String>,
    request: &mut tiny_http::Request,
    backend: &dyn Backend,
) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    match path {
        "/health" => {
            json_ok(&serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "status": "ok",
                "mode": "embedded"
            }))
        }

        "/sessions" => json_ok(&backend.list_sessions()),

        "/messages" => {
            let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
            let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
            match backend.get_messages(&file_path) {
                Ok(messages) => json_ok(&messages),
                Err(_) => json_error(404, "not found"),
            }
        }

        "/stop" => {
            let pid: u32 = query
                .get("pid")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            if pid == 0 {
                return json_error(400, "missing or invalid pid");
            }
            match backend.kill_pid(pid) {
                Ok(()) => json_ok(&serde_json::json!({"ok": true})),
                Err(e) => json_error(500, &e),
            }
        }

        "/stop_workspace" => {
            let workspace = query
                .get("path")
                .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                .unwrap_or_default();
            if workspace.is_empty() {
                return json_error(400, "missing path");
            }
            match backend.kill_workspace(workspace) {
                Ok(()) => json_ok(&serde_json::json!({"ok": true})),
                Err(e) => json_error(500, &e),
            }
        }

        "/resume_session" => {
            let session_id = query
                .get("session_id")
                .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                .unwrap_or_default();
            let workspace = query
                .get("workspace_path")
                .map(|s| percent_decode_str(s).decode_utf8_lossy().to_string())
                .unwrap_or_default();
            if session_id.is_empty() || workspace.is_empty() {
                return json_error(400, "missing session_id or workspace_path");
            }
            match backend.resume_session(session_id, workspace) {
                Ok(()) => json_ok(&serde_json::json!({"ok": true})),
                Err(e) => json_error(500, &e),
            }
        }

        "/setup-status" => json_ok(&backend.check_setup()),

        "/usage_summaries" => json_ok(&backend.usage_summaries()),

        "/waiting_alerts" => json_ok(&backend.get_waiting_alerts()),

        // ── Memory & Skills ─────────────────────────────────────────────
        "/memories" => json_ok(&backend.list_memories()),

        "/memory_content" => {
            let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
            let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
            match backend.get_memory_content(&file_path) {
                Ok(content) => json_ok(&content),
                Err(_) => json_error(404, "not found"),
            }
        }

        "/memory_history" => {
            let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
            let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
            json_ok(&backend.get_memory_history(&file_path))
        }

        "/skills" => json_ok(&backend.list_skills()),

        "/skill_content" => {
            let raw_path = query.get("path").map(|s| s.as_str()).unwrap_or("");
            let file_path = percent_decode_str(raw_path).decode_utf8_lossy().to_string();
            match backend.get_skill_content(&file_path) {
                Ok(content) => json_ok(&content),
                Err(_) => json_error(404, "not found"),
            }
        }

        // ── Hooks & Sources ─────────────────────────────────────────────
        "/hooks_plan" => json_ok(&backend.get_hooks_plan()),

        "/apply_hooks" => match backend.apply_hooks() {
            Ok(()) => json_ok(&serde_json::json!({"ok": true})),
            Err(e) => json_error(500, &e),
        },

        "/remove_hooks" => match backend.remove_hooks() {
            Ok(()) => json_ok(&serde_json::json!({"ok": true})),
            Err(e) => json_error(500, &e),
        },

        "/sources_config" => json_ok(&backend.get_sources_config()),

        "/set_source_enabled" => {
            let name = query.get("name").cloned().unwrap_or_default();
            let enabled: bool = query.get("enabled").map(|s| s == "true").unwrap_or(false);
            if name.is_empty() {
                return json_error(400, "missing name");
            }
            match backend.set_source_enabled(&name, enabled) {
                Ok(()) => json_ok(&serde_json::json!({"ok": true})),
                Err(e) => json_error(500, &e),
            }
        }

        // ── Search & Audit ──────────────────────────────────────────────
        "/search" => {
            let q = query.get("q").cloned().unwrap_or_default();
            let limit: usize = query
                .get("limit")
                .and_then(|s| s.parse().ok())
                .unwrap_or(50);
            json_ok(&backend.search_sessions(&q, limit))
        }

        "/audit" => json_ok(&backend.get_audit_events()),

        "/audit/pattern-info" => {
            let (version, path) = crate::pattern_update::get_patterns_info();
            json_ok(&serde_json::json!({"version": version, "path": path}))
        }

        "/audit/check-update" => {
            let msg = crate::pattern_update::check_update_now();
            json_ok(&serde_json::json!({"message": msg}))
        }

        // ── Daily Reports ───────────────────────────────────────────────
        "/daily_report" => {
            let date = query.get("date").cloned().unwrap_or_default();
            match backend.get_daily_report(&date) {
                Ok(report) => json_ok(&report),
                Err(e) => json_error(500, &e),
            }
        }

        "/daily_report_stats" => {
            let from = query.get("from").cloned().unwrap_or_default();
            let to = query.get("to").cloned().unwrap_or_default();
            json_ok(&backend.list_daily_report_stats(&from, &to))
        }

        "/daily_report/generate" => {
            let date = query.get("date").cloned().unwrap_or_default();
            match backend.generate_daily_report(&date) {
                Ok(report) => json_ok(&report),
                Err(e) => json_error(404, &e),
            }
        }

        "/daily_report/ai_summary" => {
            let date = query.get("date").cloned().unwrap_or_default();
            match backend.generate_daily_report_ai_summary(&date) {
                Ok(summary) => json_ok(&summary),
                Err(e) => json_error(500, &e),
            }
        }

        "/daily_report/lessons" => {
            let date = query.get("date").cloned().unwrap_or_default();
            match backend.generate_daily_report_lessons(&date) {
                Ok(lessons) => json_ok(&lessons),
                Err(e) => json_error(500, &e),
            }
        }

        "/daily_report/append_lesson" => {
            let mut body_bytes = Vec::new();
            let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
            match serde_json::from_slice::<Lesson>(&body_bytes) {
                Ok(lesson) => match backend.append_lesson_to_claude_md(&lesson) {
                    Ok(()) => json_ok(&serde_json::json!({})),
                    Err(e) => json_error(500, &e),
                },
                Err(e) => json_error(400, &format!("invalid lesson: {e}")),
            }
        }

        // ── LLM Provider ────────────────────────────────────────────────
        "/llm/providers" => json_ok(&backend.list_llm_providers()),

        "/llm/config" => {
            if request.method() == &tiny_http::Method::Get {
                json_ok(&backend.get_llm_config())
            } else {
                // POST: update config
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<LlmConfig>(&body_bytes) {
                    Ok(cfg) => match backend.set_llm_config(cfg) {
                        Ok(()) => json_ok(&serde_json::json!({})),
                        Err(e) => json_error(500, &e),
                    },
                    Err(e) => json_error(400, &format!("invalid config: {e}")),
                }
            }
        }

        // ── Auto-resume config ──────────────────────────────────────────
        "/auto_resume_config" => {
            if request.method() == &tiny_http::Method::Get {
                json_ok(&backend.get_auto_resume_config())
            } else {
                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                match serde_json::from_slice::<claw_fleet_core::auto_resume::AutoResumeConfig>(&body_bytes) {
                    Ok(cfg) => match backend.set_auto_resume_config(cfg) {
                        Ok(()) => json_ok(&serde_json::json!({})),
                        Err(e) => json_error(500, &e),
                    },
                    Err(e) => json_error(400, &format!("invalid config: {e}")),
                }
            }
        }

        // ── Per-source account/usage ────────────────────────────────────
        _ if path.starts_with("/sources/") => {
            let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
            if parts.len() == 3 {
                let source_name = parts[1];
                let kind = parts[2];
                // These are async on the Backend trait — block on them.
                let rt = tokio::runtime::Handle::try_current();
                match kind {
                    "account" => {
                        let fut = backend.source_account(source_name);
                        let result = match rt {
                            Ok(handle) => {
                                // We're inside a Tauri tokio runtime — use block_in_place
                                tokio::task::block_in_place(|| {
                                    handle.block_on(fut)
                                })
                            }
                            Err(_) => futures::executor::block_on(fut),
                        };
                        match result {
                            Ok(val) => json_ok(&val),
                            Err(e) => json_error(404, &e),
                        }
                    }
                    "usage" => {
                        let fut = backend.source_usage(source_name);
                        let result = match rt {
                            Ok(handle) => {
                                tokio::task::block_in_place(|| {
                                    handle.block_on(fut)
                                })
                            }
                            Err(_) => futures::executor::block_on(fut),
                        };
                        match result {
                            Ok(val) => json_ok(&val),
                            Err(e) => json_error(404, &e),
                        }
                    }
                    _ => json_error(404, "unknown endpoint"),
                }
            } else {
                json_error(404, "not found")
            }
        }

        _ => json_error(404, "not found"),
    }
}

// ── SSE handler ─────────────────────────────────────────────────────────────

fn handle_sse(request: tiny_http::Request, sse: &SseBroadcaster) {
    // Respond with SSE headers, then upgrade to keep the connection open.
    let response = tiny_http::Response::empty(200)
        .with_header("Content-Type: text/event-stream".parse::<tiny_http::Header>().unwrap())
        .with_header("Cache-Control: no-cache".parse::<tiny_http::Header>().unwrap())
        .with_header("Connection: keep-alive".parse::<tiny_http::Header>().unwrap())
        .with_header("Access-Control-Allow-Origin: *".parse::<tiny_http::Header>().unwrap());

    let mut stream = request.upgrade("sse", response);

    // Send initial heartbeat
    let _ = stream.write_all(b": connected\n\n");
    let _ = stream.flush();

    sse.add_client(Box::new(stream));
}

// ── Helpers ─────────────────────────────────────────────────────────────────

fn json_ok<T: Serialize>(data: &T) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::to_string(data).unwrap_or_else(|_| "null".into());
    tiny_http::Response::from_string(body).with_status_code(200)
}

fn json_error(status: u16, msg: &str) -> tiny_http::Response<std::io::Cursor<Vec<u8>>> {
    let body = serde_json::json!({"error": msg}).to_string();
    tiny_http::Response::from_string(body).with_status_code(status)
}

fn parse_query_full(query_str: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
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

// ── Landing page HTML ───────────────────────────────────────────────────────

fn build_landing_page(token: &str) -> String {
    // The deep link includes the token. The app will reconstruct the base URL
    // from the page's origin (passed via JavaScript).
    let deep_link = format!("claw-fleet://connect?token={token}");

    format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Claw Fleet — Connect</title>
<style>
  * {{ margin: 0; padding: 0; box-sizing: border-box; }}
  body {{
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    background: #0f0f1a;
    color: #e0e0e0;
    min-height: 100vh;
    display: flex;
    align-items: center;
    justify-content: center;
  }}
  .container {{
    text-align: center;
    padding: 40px 24px;
    max-width: 400px;
  }}
  .logo {{ font-size: 36px; font-weight: 800; margin-bottom: 8px; }}
  .logo span {{ color: #6366f1; }}
  .subtitle {{ color: #888; font-size: 14px; margin-bottom: 32px; }}
  .open-btn {{
    display: inline-block;
    background: #6366f1;
    color: #fff;
    padding: 14px 32px;
    border-radius: 12px;
    font-size: 16px;
    font-weight: 600;
    text-decoration: none;
    margin-bottom: 24px;
    transition: background 0.15s;
  }}
  .open-btn:hover {{ background: #4f46e5; }}
  .divider {{
    display: flex;
    align-items: center;
    gap: 12px;
    margin: 24px 0;
    color: #555;
    font-size: 12px;
  }}
  .divider::before, .divider::after {{
    content: '';
    flex: 1;
    height: 1px;
    background: #333;
  }}
  .download-section {{ margin-top: 8px; }}
  .download-title {{ font-size: 13px; color: #aaa; margin-bottom: 16px; }}
  .store-links {{ display: flex; gap: 12px; justify-content: center; flex-wrap: wrap; }}
  .store-btn {{
    display: inline-flex;
    align-items: center;
    gap: 8px;
    background: #1a1a2e;
    border: 1px solid #333;
    color: #e0e0e0;
    padding: 10px 18px;
    border-radius: 10px;
    font-size: 13px;
    font-weight: 500;
    text-decoration: none;
    transition: border-color 0.15s;
  }}
  .store-btn:hover {{ border-color: #6366f1; }}
  .store-btn.disabled {{
    opacity: 0.4;
    pointer-events: none;
  }}
  .badge {{ font-size: 10px; color: #888; }}
  .footer {{
    margin-top: 40px;
    font-size: 11px;
    color: #555;
  }}
</style>
</head>
<body>
<div class="container">
  <div class="logo">Claw <span>Fleet</span></div>
  <p class="subtitle">Monitor your AI agents from anywhere</p>

  <a href="{deep_link}" class="open-btn">Open in App</a>

  <div class="divider">Don't have the app?</div>

  <div class="download-section">
    <p class="download-title">Download Claw Fleet Mobile</p>
    <div class="store-links">
      <a href="https://github.com/hoveychen/claw-fleet/releases/latest" class="store-btn">
        <span>Android APK</span>
        <span class="badge">GitHub</span>
      </a>
      <a href="#" class="store-btn disabled">
        <span>Google Play</span>
        <span class="badge">Coming Soon</span>
      </a>
      <a href="#" class="store-btn disabled">
        <span>App Store</span>
        <span class="badge">Coming Soon</span>
      </a>
    </div>
  </div>

  <p class="footer">
    This page is served by your Claw Fleet Desktop.<br>
    The connection is end-to-end encrypted via Cloudflare Tunnel.
  </p>
</div>

<script>
  // Build deep link with the actual origin (tunnel URL) so the app knows where to connect.
  var deepLink = "claw-fleet://connect?token={token}&url=" + encodeURIComponent(window.location.origin);
  // Update the "Open in App" button with the correct deep link.
  document.querySelector('.open-btn').href = deepLink;
  // Auto-try the deep link on mobile devices.
  if (/Android|iPhone|iPad/i.test(navigator.userAgent)) {{
    window.location.href = deepLink;
  }}
</script>
</body>
</html>"##
    )
}
