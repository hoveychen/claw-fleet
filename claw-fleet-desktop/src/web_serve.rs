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

/// One frontend file, resolved by whoever owns the bundle.
pub struct StaticAsset {
    pub bytes: Vec<u8>,
    pub mime: String,
}

/// Resolves a request path to a frontend file.
///
/// Behind this is Tauri's own `AssetResolver`, which serves the very bundle the
/// desktop webview loads — so the browser gets the same `vite build` output
/// with nothing embedded twice and no second build step. Kept as a closure
/// rather than an `AppHandle` so this module stays free of Tauri types and the
/// tests can inject a fake bundle.
pub type AssetSource = Arc<dyn Fn(&str) -> Option<StaticAsset> + Send + Sync>;

type Resp = Response<Cursor<Vec<u8>>>;

/// Worker threads serving requests. A transcript read can take seconds on a
/// large session; with a single thread that would stall every other request on
/// the page (the board polls while the reader loads), so requests are handled
/// off a small pool rather than in the accept loop.
const WORKERS: usize = 4;

/// Default port for the web build. Fixed rather than ephemeral so the URL is
/// memorable and survives restarts.
pub const DEFAULT_PORT: u16 = 4571;

const CONFIG_FILE_NAME: &str = "web-access.json";

/// Persisted settings for the browser front door.
///
/// No credential of any kind lives here. The door does not authenticate: the
/// boss runs an auth gateway in front of anything exposed, so a second scheme
/// inside would only be a thing to keep in sync. What this file still decides
/// is *whether* and *where* the port opens.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebAccessConfig {
    /// Whether the front door opens at all.
    ///
    /// Off by default. The port can start agent sessions, so switching it on is
    /// the user's decision rather than something an app update does for them.
    /// The config file is still written on first run, so turning it on is a
    /// one-word edit rather than something to be told about.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Bind `0.0.0.0` instead of loopback, so a phone or another machine can
    /// reach it.
    ///
    /// Off by default, and a *separate* opt-in from `enabled` precisely because
    /// nothing here authenticates: switching this on publishes routes that can
    /// start agent sessions to everything that can route to this host. Only
    /// turn it on behind the auth gateway.
    #[serde(default)]
    pub bind_lan: bool,
}

fn default_enabled() -> bool {
    false
}

impl Default for WebAccessConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_lan: false,
        }
    }
}

fn config_path() -> Option<std::path::PathBuf> {
    claw_fleet_core::session::get_fleet_dir().map(|d| d.join(CONFIG_FILE_NAME))
}

/// Read the config, writing a default one out on first use so the switch is
/// discoverable as a file rather than something to be told about.
///
/// `load_preserving` distinguishes "no file yet" from "file is there but
/// unreadable this instant"; only the former is worth materialising, since
/// overwriting on a transient read error would discard real settings.
pub fn load_config() -> WebAccessConfig {
    use claw_fleet_core::atomic_json::JsonLoad;
    let Some(path) = config_path() else {
        return WebAccessConfig::default();
    };
    match claw_fleet_core::atomic_json::load_preserving(&path) {
        JsonLoad::Loaded(cfg) => cfg,
        JsonLoad::Missing | JsonLoad::Corrupt => {
            let cfg = WebAccessConfig::default();
            let _ = save_config(&cfg);
            cfg
        }
        JsonLoad::Unreadable => WebAccessConfig::default(),
    }
}

/// Persist at 0600 — it decides whether a port that can start agent sessions
/// opens at all.
pub fn save_config(cfg: &WebAccessConfig) -> std::io::Result<()> {
    let path = config_path()
        .ok_or_else(|| std::io::Error::other("no fleet dir"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_vec_pretty(cfg).map_err(std::io::Error::other)?;
    claw_fleet_core::atomic_json::write_atomic(&path, &json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

struct Ctx {
    backend: SharedBackend,
    /// Runtime for the two `Backend` methods that return futures
    /// (`account_info` / `source_usage`). Built once — `Runtime::new()` spins
    /// up worker threads, so per-request construction would be wasteful.
    rt: tokio::runtime::Runtime,
    /// Frontend bundle, when one is available. `None` leaves the port as a
    /// pure data API (what the tests drive).
    assets: Option<AssetSource>,
}

/// Bind the front door and serve it from background threads.
///
/// Binds loopback unless `bindLan` is set in `~/.fleet/web-access.json`.
pub fn start(
    backend: SharedBackend,
    port: u16,
    assets: Option<AssetSource>,
) -> std::io::Result<u16> {
    start_with_config(backend, port, load_config(), assets)
}

/// `start` with the config supplied rather than read from disk — the seam the
/// tests drive, so they never touch the real `~/.fleet/web-access.json`.
pub fn start_with_config(
    backend: SharedBackend,
    port: u16,
    config: WebAccessConfig,
    assets: Option<AssetSource>,
) -> std::io::Result<u16> {
    if !config.enabled {
        return Err(std::io::Error::other(
            "disabled — set \"enabled\": true in ~/.fleet/web-access.json to open it",
        ));
    }
    let host = if config.bind_lan { "0.0.0.0" } else { "127.0.0.1" };
    let server = Server::http((host, port))
        .map_err(|e| std::io::Error::other(format!("web front door bind failed: {e}")))?;
    let bound = server
        .server_addr()
        .to_ip()
        .map(|a| a.port())
        .unwrap_or(port);

    let rt = tokio::runtime::Runtime::new()?;
    let ctx = Arc::new(Ctx {
        backend,
        rt,
        assets,
    });
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

/// Serve a file out of the frontend bundle.
///
/// `/` maps to `/index.html`; anything the bundle doesn't have 404s rather than
/// falling back to index, so a mistyped asset path stays visible as a missing
/// file instead of silently returning HTML.
fn serve_static(ctx: &Ctx, path: &str) -> Resp {
    let Some(assets) = ctx.assets.as_ref() else {
        return error(404, "no such route");
    };
    let wanted = if path == "/" { "/index.html" } else { path };
    match assets(wanted) {
        Some(asset) => {
            let header: Header = format!("Content-Type: {}", asset.mime)
                .parse()
                .unwrap_or_else(|_| json_header());
            Response::from_data(asset.bytes).with_header(header)
        }
        None => error(404, "no such file"),
    }
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

    let method = request.method().as_str().to_string();

    // No auth gate. Deliberate: the door binds loopback unless explicitly told
    // otherwise, and anything exposed sits behind the boss's auth gateway. See
    // `WebAccessConfig`.
    let response = match (method.as_str(), path) {
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

        // Anything not a data route is a request for the frontend bundle.
        ("GET", _) => serve_static(ctx, path),

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

    /// Boot a front door on an OS-assigned port, never touching the real
    /// `~/.fleet/web-access.json`.
    fn serve() -> String {
        serve_with_assets(None)
    }

    fn serve_with_assets(assets: Option<AssetSource>) -> String {
        let backend: SharedBackend = Arc::new(RwLock::new(Box::new(crate::NullBackend)));
        let port = start_with_config(
            backend,
            0,
            WebAccessConfig { enabled: true, bind_lan: false },
            assets,
        )
        .expect("front door must bind");
        format!("http://127.0.0.1:{port}")
    }

    /// A two-file stand-in for the real `vite build` bundle.
    fn fake_bundle() -> Option<AssetSource> {
        Some(Arc::new(|path: &str| match path {
            "/index.html" => Some(StaticAsset {
                bytes: b"<html>board</html>".to_vec(),
                mime: "text/html".to_string(),
            }),
            "/assets/app.js" => Some(StaticAsset {
                bytes: b"console.log(1)".to_vec(),
                mime: "text/javascript".to_string(),
            }),
            _ => None,
        }))
    }


    /// End-to-end over a real socket: the point of this module is that an HTTP
    /// request reaches a `Backend` method and its return value comes back as
    /// JSON. The route table alone can't show that, so this drives the server
    /// with `NullBackend`, whose per-method returns happen to cover all four
    /// response shapes the dispatcher produces.
    #[test]
    fn http_requests_reach_the_backend() {
        let base = serve();
        let client = reqwest::blocking::Client::new();
        let get = |path: String| client.get(path).send().unwrap();

        // Ok(Vec) → 200 + serialized JSON.
        let sessions = get(format!("{base}/sessions"));
        assert_eq!(sessions.status(), 200);
        assert_eq!(sessions.text().unwrap(), "[]");

        // Err(String) → 500 carrying the backend's own message, which is what
        // liveProxy's callProbe re-throws to the UI.
        let messages = get(format!("{base}/messages?path=/x.jsonl"));
        assert_eq!(messages.status(), 500);
        assert_eq!(messages.text().unwrap(), "backend not ready");

        // Option::None → 200 + `null` (not 404): the frontend distinguishes
        // "no live thinking" from a failed request.
        let thinking = get(format!("{base}/live_thinking?session_id=abc"));
        assert_eq!(thinking.status(), 200);
        assert_eq!(thinking.text().unwrap(), "null");

        // Result<(), _> route: body is discarded, status carries the outcome.
        assert_eq!(get(format!("{base}/interrupt?pid=123")).status(), 500);

        // Query parsing is enforced before the backend is touched.
        assert_eq!(get(format!("{base}/interrupt?pid=notanumber")).status(), 400);

        // Unmapped paths must not fall through to anything.
        assert_eq!(get(format!("{base}/settings")).status(), 404);

        // A nested source name is rejected at the router, not passed along.
        assert_eq!(get(format!("{base}/sources/a/b/usage")).status(), 404);
    }





    /// Opening a port that can start agent sessions must never be something an
    /// app update does on the user's behalf — it stays off until they say so,
    /// whether the field is absent or the struct is defaulted.
    #[test]
    fn the_front_door_is_off_until_switched_on() {
        assert!(!WebAccessConfig::default().enabled);

        let from_empty: WebAccessConfig = serde_json::from_str("{}").unwrap();
        assert!(!from_empty.enabled, "a config with no `enabled` key must stay shut");

        let explicit: WebAccessConfig = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert!(explicit.enabled, "an explicit opt-in must still be honoured");
    }

    /// LAN exposure is a second, separate opt-in: switching the door on must
    /// not also publish it beyond loopback.
    #[test]
    fn enabling_does_not_imply_lan_exposure() {
        let opted_in: WebAccessConfig = serde_json::from_str(r#"{"enabled": true}"#).unwrap();
        assert!(!opted_in.bind_lan);
    }

    #[test]
    fn disabled_config_refuses_to_bind() {
        let backend: SharedBackend = Arc::new(RwLock::new(Box::new(crate::NullBackend)));
        let result = start_with_config(
            backend,
            0,
            WebAccessConfig { enabled: false, bind_lan: false },
            None,
        );
        assert!(result.is_err(), "disabled must not open a port");
    }



    #[test]
    fn browser_gets_the_bundle() {
        let base = serve_with_assets(fake_bundle());
        let client = reqwest::blocking::Client::new();
        let get = |path: String| {
            client.get(path).header("Accept", "text/html").send().unwrap()
        };

        // `/` resolves to index.html.
        let index = get(base.clone());
        assert_eq!(index.status(), 200);
        assert!(index.text().unwrap().contains("board"));

        // Nested asset paths pass through verbatim, mime included.
        let js = get(format!("{base}/assets/app.js"));
        assert_eq!(js.status(), 200);
        assert_eq!(js.headers()["content-type"], "text/javascript");

        // A file the bundle doesn't have 404s rather than falling back to
        // index.html — a mistyped asset must stay visibly missing.
        assert_eq!(get(format!("{base}/assets/nope.js")).status(), 404);
    }

    /// The door authenticates nothing, on purpose — auth lives in the gateway
    /// in front of it. Pinned as a test so it reads as a decision rather than
    /// an oversight: if someone later adds a gate here, this fails and they
    /// have to confirm the deployment story changed.
    #[test]
    fn no_credential_is_required() {
        let base = serve();
        let client = reqwest::blocking::Client::new();
        // Bare request, no cookie and no Authorization header.
        let resp = client.get(format!("{base}/sessions")).send().unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().unwrap(), "[]");
    }

    /// Data routes must win over the bundle: were the static arm to shadow one,
    /// the page would receive HTML where it expects JSON.
    #[test]
    fn data_routes_are_not_shadowed_by_static() {
        let base = serve_with_assets(fake_bundle());
        let resp = reqwest::blocking::Client::new()
            .get(format!("{base}/sessions"))
            .header("Accept", "text/html")
            .send()
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().unwrap(), "[]");
    }

}
