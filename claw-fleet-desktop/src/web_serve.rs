//! HTTP front door for the browser build of the app.
//!
//! The frontend is already a plain web app (React + vite) — the only thing
//! binding it to Tauri is `invoke()`. `app/mock/liveProxy.ts` proved the whole
//! board renders from HTTP alone, but it routes through the *vite dev server's*
//! proxy, and `vite build` ignores `server.proxy`, so that path cannot ship.
//! This module is that proxy's shippable half.
//!
//! Why it lives in the app process rather than reusing `fleet serve`:
//! `hooks_server::serve` builds its own `ServeCtx` internally (session
//! snapshot, report store, search index, audit history), so embedding it would
//! give the browser a *second* copy of that state and the two views would
//! drift. Here we borrow the app's own `Arc<RwLock<Box<dyn Backend>>>`, so the
//! page reads exactly what the desktop window reads — including the
//! `connect_remote` swap, which this picks up for free since the lock is
//! shared rather than the backend cloned out of it.
//!
//! Paths mirror `claw_fleet_core::routes` (and therefore `remote.rs`), which is
//! what `LIVE_ROUTES` in liveProxy.ts was written against. Keeping them
//! identical is what lets the frontend's mapping table stay a single table
//! instead of one per transport.

use std::collections::HashMap;
use std::io::Cursor;
use std::sync::{Arc, RwLock};

use claw_fleet_core::backend::Backend;
use serde::Deserialize;
use serde::Serialize;
use tiny_http::{Header, Request, Response, Server};

/// Shared handle on the app's live backend — the same `Arc` `AppState` holds.
pub type SharedBackend = Arc<RwLock<Box<dyn Backend>>>;

type Resp = Response<Cursor<Vec<u8>>>;

/// Worker threads serving requests. A transcript read can take seconds on a
/// large session; with a single thread that would stall every other request on
/// the page (the board polls while the reader loads), so requests are handled
/// off a small pool rather than in the accept loop.
const WORKERS: usize = 4;

/// Default port for the web build. Fixed rather than ephemeral so the URL is
/// memorable and survives restarts.
pub const DEFAULT_PORT: u16 = 4571;

struct Ctx {
    backend: SharedBackend,
    /// Runtime for the two `Backend` methods that return futures
    /// (`account_info` / `source_usage`). Built once — `Runtime::new()` spins
    /// up worker threads, so per-request construction would be wasteful.
    rt: tokio::runtime::Runtime,
}

/// Bind the front door and serve it from background threads.
///
/// Binds loopback only: this port answers every request without auth for now,
/// so it must not be reachable from the LAN. The password gate (P2) is what
/// unlocks a wider bind.
pub fn start(backend: SharedBackend, port: u16) -> std::io::Result<u16> {
    let server = Server::http(("127.0.0.1", port))
        .map_err(|e| std::io::Error::other(format!("web front door bind failed: {e}")))?;
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(port);

    let rt = tokio::runtime::Runtime::new()?;
    let ctx = Arc::new(Ctx { backend, rt });
    let server = Arc::new(server);

    for _ in 0..WORKERS {
        let server = server.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            // `recv()` errors only once the server is torn down; the app owns
            // the server for its whole lifetime, so treat it as terminal.
            while let Ok(request) = server.recv() {
                handle(&ctx, request);
            }
        });
    }

    Ok(bound)
}

// ── Responses ───────────────────────────────────────────────────────────────

fn json_header() -> Header {
    // Parsing a literal that is known-good; the unwrap cannot fire.
    "Content-Type: application/json".parse().unwrap()
}

fn json_body<T: Serialize>(value: &T) -> Resp {
    match serde_json::to_vec(value) {
        Ok(bytes) => Response::from_data(bytes).with_header(json_header()),
        Err(e) => error(500, &format!("serialize failed: {e}")),
    }
}

/// `Result`-returning backend methods: `Ok` → JSON, `Err` → 500 with the
/// message as text. liveProxy's `callProbe` reads the body on !ok and throws
/// it, so the frontend surfaces the backend's own wording.
fn json_result<T: Serialize>(value: Result<T, String>) -> Resp {
    match value {
        Ok(v) => json_body(&v),
        Err(e) => error(500, &e),
    }
}

/// Routes the frontend marks `empty: true` — it discards the body, so only the
/// status carries meaning.
fn empty_result(value: Result<(), String>) -> Resp {
    match value {
        Ok(()) => Response::from_data(Vec::new()),
        Err(e) => error(500, &e),
    }
}

fn error(status: u16, message: &str) -> Resp {
    Response::from_data(message.as_bytes().to_vec()).with_status_code(status)
}

// ── Request parsing ─────────────────────────────────────────────────────────

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for pair in query.split('&').filter(|s| !s.is_empty()) {
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        out.insert(decode(k), decode(v));
    }
    out
}

/// Percent-decode, then `+` → space. Query strings reach us encoded by
/// `URLSearchParams`, which uses `+` for spaces — a bare percent-decode would
/// leave a literal `+` inside a workspace path that has one.
fn decode(raw: &str) -> String {
    percent_encoding::percent_decode_str(&raw.replace('+', " "))
        .decode_utf8_lossy()
        .to_string()
}

fn read_body(request: &mut Request) -> Result<String, String> {
    let mut body = String::new();
    request
        .as_reader()
        .read_to_string(&mut body)
        .map_err(|e| format!("read body: {e}"))?;
    Ok(body)
}

fn parse_body<T: for<'de> Deserialize<'de>>(request: &mut Request) -> Result<T, String> {
    let body = read_body(request)?;
    serde_json::from_str(&body).map_err(|e| format!("parse body: {e}"))
}

// ── POST bodies ─────────────────────────────────────────────────────────────
// `#[serde(rename_all = "camelCase")]` matches the probe's own request shapes,
// which the frontend already targets — snake_case here would 400 every spawn.

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnBody {
    workspace_path: String,
    prompt: String,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    permission_mode: Option<String>,
    #[serde(default)]
    tool: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct EnqueueBody {
    session_id: String,
    workspace_path: String,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResumeBody {
    session_id: String,
    workspace_path: String,
    #[serde(default)]
    prompt: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    effort: Option<String>,
    #[serde(default)]
    permission_mode: Option<String>,
    #[serde(default = "default_source")]
    agent_source: String,
}

fn default_source() -> String {
    "claude".to_string()
}

// ── Dispatch ────────────────────────────────────────────────────────────────

fn handle(ctx: &Ctx, mut request: Request) {
    let url = request.url().to_string();
    let (path, query_str) = url.split_once('?').unwrap_or((url.as_str(), ""));
    let query = parse_query(query_str);
    let get = |key: &str| query.get(key).cloned().unwrap_or_default();
    // Absent and empty are the same thing for the optional params: the
    // frontend's `q()` drops nulls, and `URLSearchParams` skips empty values.
    let opt = |key: &str| query.get(key).filter(|v| !v.is_empty()).cloned();

    let response = match (request.method().as_str(), path) {
        ("GET", "/health") => json_body(&serde_json::json!({ "ok": true })),

        ("GET", claw_fleet_core::routes::SESSIONS) => {
            json_body(&ctx.backend.read().unwrap().list_sessions())
        }

        // One path, two backend methods: `?tail=` picks the windowed read.
        ("GET", claw_fleet_core::routes::MESSAGES) => {
            let path_arg = get("path");
            let backend = ctx.backend.read().unwrap();
            match opt("tail").and_then(|t| t.parse::<usize>().ok()) {
                Some(tail) => json_result(backend.get_messages_tail(&path_arg, tail)),
                None => json_result(backend.get_messages(&path_arg)),
            }
        }

        ("GET", claw_fleet_core::routes::TOOL_RESULT) => json_result(
            ctx.backend
                .read()
                .unwrap()
                .get_tool_result_full(&get("path"), &get("tool_use_id")),
        ),

        ("GET", claw_fleet_core::routes::SKILL_HISTORY) => {
            json_result(ctx.backend.read().unwrap().get_skill_history(&get("path")))
        }

        ("GET", claw_fleet_core::routes::WORKFLOW_TREES) => {
            json_result(ctx.backend.read().unwrap().get_workflow_trees(&get("path")))
        }

        ("GET", claw_fleet_core::routes::DSH_TOKEN_BREAKDOWN) => json_result(
            ctx.backend
                .read()
                .unwrap()
                .get_dsh_token_breakdown(&get("path")),
        ),

        ("GET", claw_fleet_core::routes::DSH_SESSION_COST) => json_result(
            ctx.backend
                .read()
                .unwrap()
                .get_dsh_session_cost(&get("path")),
        ),

        ("GET", claw_fleet_core::routes::DSH_MODELS) => {
            json_result(ctx.backend.read().unwrap().dsh_models())
        }

        ("GET", claw_fleet_core::routes::SESSION_DECISIONS) => {
            json_body(&ctx.backend.read().unwrap().list_session_decisions(
                &get("session_id"),
                opt("jsonl_path").as_deref(),
            ))
        }

        ("GET", claw_fleet_core::routes::LIVE_THINKING) => json_body(
            &ctx.backend
                .read()
                .unwrap()
                .read_live_thinking(&get("session_id")),
        ),

        ("GET", claw_fleet_core::routes::TASK_PLANS) => json_body(
            &ctx.backend
                .read()
                .unwrap()
                .get_task_plans(&get("path"), opt("session").as_deref()),
        ),

        ("GET", claw_fleet_core::routes::PLAN_FOREST) => {
            json_body(&ctx.backend.read().unwrap().get_plan_forest(&get("path")))
        }

        ("GET", claw_fleet_core::routes::EXPLORER_EXTERNAL_FILE) => {
            json_result(ctx.backend.read().unwrap().read_external_file(&get("path")))
        }

        ("GET", claw_fleet_core::routes::TODAY_USAGE) => {
            json_body(&ctx.backend.read().unwrap().today_usage())
        }

        ("GET", claw_fleet_core::routes::SOURCES_CONFIG) => {
            json_body(&ctx.backend.read().unwrap().get_sources_config())
        }

        // The two future-returning methods. The read guard is dropped before
        // `block_on` — these futures wrap network calls, and holding the lock
        // across one would block `connect_remote`'s write for its duration.
        ("GET", claw_fleet_core::routes::SOURCES_CLAUDE_ACCOUNT) => {
            let future = ctx.backend.read().unwrap().account_info();
            json_result(ctx.rt.block_on(future))
        }

        ("GET", p) if is_source_usage(p) => match source_name(p) {
            Some(source) => {
                let future = ctx.backend.read().unwrap().source_usage(&source);
                json_result(ctx.rt.block_on(future))
            }
            None => error(404, "unknown source path"),
        },

        ("POST", claw_fleet_core::routes::SPAWN_SESSION) => {
            match parse_body::<SpawnBody>(&mut request) {
                Ok(b) => json_result(ctx.backend.read().unwrap().spawn_new_session(
                    b.workspace_path,
                    b.prompt,
                    b.model,
                    b.effort,
                    b.permission_mode,
                    b.tool,
                )),
                Err(e) => error(400, &e),
            }
        }

        ("POST", claw_fleet_core::routes::ENQUEUE_MESSAGE) => {
            match parse_body::<EnqueueBody>(&mut request) {
                Ok(b) => empty_result(ctx.backend.read().unwrap().enqueue_message(
                    b.session_id,
                    b.workspace_path,
                    b.text,
                )),
                Err(e) => error(400, &e),
            }
        }

        ("POST", claw_fleet_core::routes::RESUME_SESSION) => {
            match parse_body::<ResumeBody>(&mut request) {
                Ok(b) => empty_result(ctx.backend.read().unwrap().resume_session(
                    b.session_id,
                    b.workspace_path,
                    b.prompt,
                    b.model,
                    b.effort,
                    b.permission_mode,
                    b.agent_source,
                )),
                Err(e) => error(400, &e),
            }
        }

        ("GET", claw_fleet_core::routes::INTERRUPT_AGENT_SESSION) => empty_result(
            ctx.backend
                .read()
                .unwrap()
                .interrupt_agent_session(get("path")),
        ),

        ("GET", claw_fleet_core::routes::INTERRUPT) => match get("pid").parse::<u32>() {
            Ok(pid) => empty_result(ctx.backend.read().unwrap().interrupt_pid(pid)),
            Err(_) => error(400, "pid must be a number"),
        },

        ("GET", claw_fleet_core::routes::STOP) => match get("pid").parse::<u32>() {
            Ok(pid) => empty_result(ctx.backend.read().unwrap().kill_pid(pid)),
            Err(_) => error(400, "pid must be a number"),
        },

        _ => error(404, "no such route"),
    };

    let _ = request.respond(response);
}

/// `/sources/<name>/usage` — the one templated path in the set.
fn is_source_usage(path: &str) -> bool {
    path.starts_with(claw_fleet_core::routes::SOURCES_PREFIX) && path.ends_with("/usage")
}

fn source_name(path: &str) -> Option<String> {
    let rest = path.strip_prefix(claw_fleet_core::routes::SOURCES_PREFIX)?;
    let name = rest.strip_suffix("/usage")?;
    // Guard against `/sources/a/b/usage` reaching the backend as "a/b".
    if name.is_empty() || name.contains('/') {
        return None;
    }
    Some(name.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_decodes_percent_and_plus() {
        let q = parse_query("path=%2FUsers%2Fa+b%2Fx.jsonl&tail=200");
        assert_eq!(q.get("path").unwrap(), "/Users/a b/x.jsonl");
        assert_eq!(q.get("tail").unwrap(), "200");
    }

    #[test]
    fn empty_query_yields_no_keys() {
        assert!(parse_query("").is_empty());
    }

    #[test]
    fn source_usage_path_is_recognized() {
        assert!(is_source_usage("/sources/claude/usage"));
        assert_eq!(source_name("/sources/codex/usage").unwrap(), "codex");
    }

    #[test]
    fn source_usage_rejects_nested_names() {
        // A nested name would otherwise reach the backend as "a/b".
        assert_eq!(source_name("/sources/a/b/usage"), None);
        assert_eq!(source_name("/sources//usage"), None);
    }

    #[test]
    fn account_path_is_not_mistaken_for_usage() {
        assert!(!is_source_usage(
            claw_fleet_core::routes::SOURCES_CLAUDE_ACCOUNT
        ));
    }

    /// End-to-end over a real socket: the point of this module is that an HTTP
    /// request reaches a `Backend` method and its return value comes back as
    /// JSON. The route table alone can't show that, so this drives the server
    /// with `NullBackend`, whose per-method returns happen to cover all four
    /// response shapes the dispatcher produces.
    #[test]
    fn http_requests_reach_the_backend() {
        let backend: SharedBackend = Arc::new(RwLock::new(Box::new(crate::NullBackend)));
        // Port 0: the OS picks a free one, so the test never collides with a
        // running app on DEFAULT_PORT.
        let port = start(backend, 0).expect("front door must bind");
        let base = format!("http://127.0.0.1:{port}");
        let client = reqwest::blocking::Client::new();

        // Ok(Vec) → 200 + serialized JSON.
        let sessions = client.get(format!("{base}/sessions")).send().unwrap();
        assert_eq!(sessions.status(), 200);
        assert_eq!(sessions.text().unwrap(), "[]");

        // Err(String) → 500 carrying the backend's own message, which is what
        // liveProxy's callProbe re-throws to the UI.
        let messages = client
            .get(format!("{base}/messages?path=/x.jsonl"))
            .send()
            .unwrap();
        assert_eq!(messages.status(), 500);
        assert_eq!(messages.text().unwrap(), "backend not ready");

        // Option::None → 200 + `null` (not 404): the frontend distinguishes
        // "no live thinking" from a failed request.
        let thinking = client
            .get(format!("{base}/live_thinking?session_id=abc"))
            .send()
            .unwrap();
        assert_eq!(thinking.status(), 200);
        assert_eq!(thinking.text().unwrap(), "null");

        // Result<(), _> route: body is discarded, status carries the outcome.
        let interrupt = client
            .get(format!("{base}/interrupt?pid=123"))
            .send()
            .unwrap();
        assert_eq!(interrupt.status(), 500);

        // Query parsing is enforced before the backend is touched.
        let bad_pid = client
            .get(format!("{base}/interrupt?pid=notanumber"))
            .send()
            .unwrap();
        assert_eq!(bad_pid.status(), 400);

        // Unmapped paths must not fall through to anything.
        let missing = client.get(format!("{base}/settings")).send().unwrap();
        assert_eq!(missing.status(), 404);

        // A nested source name is rejected at the router, not passed along.
        let nested = client
            .get(format!("{base}/sources/a/b/usage"))
            .send()
            .unwrap();
        assert_eq!(nested.status(), 404);
    }
}
