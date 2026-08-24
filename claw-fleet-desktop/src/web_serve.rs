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

use std::collections::{HashMap, HashSet};
use std::io::Cursor;
use std::sync::{Arc, Mutex, RwLock};

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

/// Cookie carrying a logged-in browser's session token.
const COOKIE_NAME: &str = "fleet_web";
const CONFIG_FILE_NAME: &str = "web-access.json";

/// Persisted settings for the browser front door.
///
/// There is deliberately no token/secret for an upstream service here: this
/// module talks to the app's own in-process backend, so — unlike the
/// `fleet serve` path — there is no admin token that could leak into the page.
/// The password guards *this* port and nothing else.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct WebAccessConfig {
    /// Whether the front door accepts logins at all.
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// Password the browser must present once per session. Generated on first
    /// load; an empty value denies every login rather than allowing any.
    #[serde(default)]
    pub password: String,
    /// Bind `0.0.0.0` instead of loopback, so a phone or another machine on the
    /// LAN can reach it. Off by default — opening a port that can start agent
    /// sessions is the user's call, not a default.
    #[serde(default)]
    pub bind_lan: bool,
}

fn default_enabled() -> bool {
    true
}

impl Default for WebAccessConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            password: String::new(),
            bind_lan: false,
        }
    }
}

fn config_path() -> Option<std::path::PathBuf> {
    claw_fleet_core::session::get_fleet_dir().map(|d| d.join(CONFIG_FILE_NAME))
}

/// Read the config, minting a password on first use.
///
/// `load_preserving` distinguishes "no file yet" from "file is there but
/// unreadable this instant". Only the former may be replaced with a freshly
/// generated password — overwriting on a transient read error would rotate the
/// password out from under a browser that is already logged in.
pub fn load_config() -> WebAccessConfig {
    use claw_fleet_core::atomic_json::JsonLoad;
    let Some(path) = config_path() else {
        return WebAccessConfig::default();
    };
    let (mut cfg, may_persist) = match claw_fleet_core::atomic_json::load_preserving(&path) {
        JsonLoad::Loaded(cfg) => (cfg, true),
        JsonLoad::Missing | JsonLoad::Corrupt => (WebAccessConfig::default(), true),
        JsonLoad::Unreadable => (WebAccessConfig::default(), false),
    };
    if cfg.password.is_empty() && may_persist {
        // 64 hex chars is what the relay pairing secret uses; reused here
        // rather than inventing a second entropy helper.
        cfg.password = claw_fleet_core::mobile_relay::generate_secret();
        let _ = save_config(&cfg);
    }
    cfg
}

/// Persist at 0600 — the password grants the right to start agent sessions.
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
    /// The password a browser must present to `/web/login`.
    password: String,
    /// Session tokens handed out to logged-in browsers. In memory only, so an
    /// app restart forces a re-login — acceptable, and it means a leaked cookie
    /// cannot outlive the process.
    sessions: Mutex<HashSet<String>>,
    /// Frontend bundle, when one is available. `None` leaves the port as a
    /// pure data API (what the tests drive).
    assets: Option<AssetSource>,
}

impl Ctx {
    /// A request is authorized when its cookie names a live session token.
    fn authorized(&self, request: &Request) -> bool {
        match session_cookie(request) {
            Some(token) => self.sessions.lock().unwrap().contains(&token),
            None => false,
        }
    }
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
/// tests drive, so they neither read nor mint the real `~/.fleet` password.
pub fn start_with_config(
    backend: SharedBackend,
    port: u16,
    config: WebAccessConfig,
    assets: Option<AssetSource>,
) -> std::io::Result<u16> {
    if !config.enabled {
        return Err(std::io::Error::other(
            "web front door disabled in web-access.json",
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
        password: config.password,
        sessions: Mutex::new(HashSet::new()),
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

/// Value of the session cookie, if the request carries one.
///
/// Browsers send every cookie for the origin in one header, so the app's own
/// cookies ride along; pick out ours by name rather than assuming position.
fn session_cookie(request: &Request) -> Option<String> {
    let header = request
        .headers()
        .iter()
        .find(|h| h.field.equiv("cookie"))?
        .value
        .as_str()
        .to_string();
    for pair in header.split(';') {
        // A valueless segment must not abort the scan — ours may follow it.
        if let Some((name, value)) = pair.trim().split_once('=') {
            if name == COOKIE_NAME {
                return Some(value.to_string());
            }
        }
    }
    None
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

#[derive(Deserialize)]
struct LoginBody {
    password: String,
}

// ── Auth ────────────────────────────────────────────────────────────────────

/// Routes reachable without a session cookie. Everything else is gated.
///
/// `/health` stays open so "is the front door up" is answerable without
/// credentials; it reveals nothing but liveness.
fn is_public(method: &str, path: &str) -> bool {
    matches!((method, path), ("GET", "/health") | ("POST", LOGIN_PATH))
}

const LOGIN_PATH: &str = "/web/login";
const LOGOUT_PATH: &str = "/web/logout";

fn login(ctx: &Ctx, request: &mut Request) -> Resp {
    let body: LoginBody = match parse_body(request) {
        Ok(b) => b,
        Err(e) => return error(400, &e),
    };
    // An empty configured password denies every attempt rather than accepting
    // any — the same invariant `hooks_server::auth` holds for an empty token.
    if ctx.password.is_empty() || body.password != ctx.password {
        return error(401, "wrong password");
    }
    let token = claw_fleet_core::mobile_relay::generate_secret();
    ctx.sessions.lock().unwrap().insert(token.clone());
    // No `Secure`: this is served over plain HTTP on loopback/LAN, and a
    // Secure cookie would simply never be sent back.
    let cookie: Header = format!("Set-Cookie: {COOKIE_NAME}={token}; HttpOnly; SameSite=Strict; Path=/")
        .parse()
        .expect("cookie header is well-formed");
    Response::from_data(b"{\"ok\":true}".to_vec())
        .with_header(json_header())
        .with_header(cookie)
}

/// Whether an unauthorized GET should be answered with the login page rather
/// than a bare 401. A navigating browser sends `Accept: text/html`; the app's
/// own `fetch` calls do not, so they still get the 401 their caller expects.
fn wants_html(request: &Request) -> bool {
    request
        .headers()
        .iter()
        .find(|h| h.field.equiv("accept"))
        .map(|h| h.value.as_str().contains("text/html"))
        .unwrap_or(false)
}

/// Served to a browser that hasn't logged in yet.
///
/// Self-contained on purpose: it must render before any of the app bundle is
/// authorized, so it cannot reference `/assets/*`.
fn login_page() -> Resp {
    let html = r#"<!doctype html>
<html lang="zh">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Fleet</title>
<style>
  :root { color-scheme: light dark; }
  body {
    margin: 0; min-height: 100vh; display: grid; place-items: center;
    background: #f4f1ea; color: #2b2b2b;
    font: 15px/1.5 -apple-system, system-ui, "Helvetica Neue", sans-serif;
  }
  form {
    background: #fffdf8; border: 1px solid #ddd6c9; border-radius: 10px;
    padding: 28px 30px; width: 288px;
    box-shadow: 0 1px 2px rgba(0,0,0,.04);
  }
  h1 { margin: 0 0 4px; font-size: 17px; font-weight: 600; letter-spacing: -.01em; }
  p { margin: 0 0 18px; font-size: 13px; color: #6b6459; }
  input, button { width: 100%; box-sizing: border-box; font: inherit; }
  input {
    padding: 9px 11px; border: 1px solid #d3ccbe; border-radius: 6px;
    background: #fff; margin-bottom: 12px;
  }
  input:focus { outline: 2px solid #b9a06b; outline-offset: -1px; border-color: #b9a06b; }
  button {
    padding: 9px 11px; border: 0; border-radius: 6px; cursor: pointer;
    background: #2b2b2b; color: #f8f6f1; font-weight: 500;
  }
  button:active { transform: translateY(.5px); }
  .err { margin: 12px 0 0; font-size: 13px; color: #b03030; min-height: 1.5em; }
  @media (prefers-color-scheme: dark) {
    body { background: #1c1b19; color: #e8e4dc; }
    form { background: #242220; border-color: #3a3733; }
    p { color: #9a938a; }
    input { background: #1c1b19; border-color: #45413b; color: inherit; }
    button { background: #e8e4dc; color: #1c1b19; }
  }
</style>
</head>
<body>
<form id="f">
  <h1>Fleet</h1>
  <p>输入访问密码</p>
  <input id="p" type="password" autocomplete="current-password" autofocus>
  <button type="submit">进入</button>
  <p class="err" id="e"></p>
</form>
<script>
document.getElementById('f').addEventListener('submit', async (ev) => {
  ev.preventDefault();
  const err = document.getElementById('e');
  err.textContent = '';
  try {
    const resp = await fetch('/web/login', {
      method: 'POST',
      headers: { 'Content-Type': 'application/json' },
      body: JSON.stringify({ password: document.getElementById('p').value }),
    });
    if (resp.ok) { location.reload(); return; }
    err.textContent = resp.status === 401 ? '密码不对' : '登录失败：' + resp.status;
  } catch (e) {
    err.textContent = '连接失败';
  }
});
</script>
</body>
</html>"#;
    let header: Header = "Content-Type: text/html; charset=utf-8".parse().unwrap();
    Response::from_data(html.as_bytes().to_vec()).with_header(header)
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

fn logout(ctx: &Ctx, request: &Request) -> Resp {
    if let Some(token) = session_cookie(request) {
        ctx.sessions.lock().unwrap().remove(&token);
    }
    let cookie: Header = format!("Set-Cookie: {COOKIE_NAME}=; HttpOnly; SameSite=Strict; Path=/; Max-Age=0")
        .parse()
        .expect("cookie header is well-formed");
    Response::from_data(b"{\"ok\":true}".to_vec())
        .with_header(json_header())
        .with_header(cookie)
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

    // Gate first: no route below should have to think about auth.
    if !is_public(&method, path) && !ctx.authorized(&request) {
        // A browser navigating here gets the password prompt; a data fetch gets
        // the 401 its caller is written to handle.
        let denied = if method == "GET" && wants_html(&request) {
            login_page()
        } else {
            error(401, "login required")
        };
        let _ = request.respond(denied);
        return;
    }

    let response = match (method.as_str(), path) {
        ("GET", "/health") => json_body(&serde_json::json!({ "ok": true })),

        ("POST", LOGIN_PATH) => login(ctx, &mut request),

        ("POST", LOGOUT_PATH) => logout(ctx, &request),

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

    const TEST_PASSWORD: &str = "correct-horse";

    /// Boot a front door on an OS-assigned port with the config supplied, so no
    /// test reads or mints the real `~/.fleet/web-access.json` password.
    fn serve(password: &str) -> String {
        serve_with_assets(password, None)
    }

    fn serve_with_assets(password: &str, assets: Option<AssetSource>) -> String {
        let backend: SharedBackend = Arc::new(RwLock::new(Box::new(crate::NullBackend)));
        let port = start_with_config(
            backend,
            0,
            WebAccessConfig {
                enabled: true,
                password: password.to_string(),
                bind_lan: false,
            },
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

    /// Log in and return the session cookie value. reqwest is built here
    /// without the `cookies` feature, so the cookie is carried by hand.
    fn login_cookie(base: &str, password: &str) -> String {
        let resp = reqwest::blocking::Client::new()
            .post(format!("{base}{LOGIN_PATH}"))
            .json(&serde_json::json!({ "password": password }))
            .send()
            .unwrap();
        assert_eq!(resp.status(), 200, "login should succeed");
        let set = resp
            .headers()
            .get("set-cookie")
            .expect("login must set a cookie")
            .to_str()
            .unwrap();
        assert!(set.contains("HttpOnly"), "cookie must be HttpOnly: {set}");
        set.split(';')
            .next()
            .unwrap()
            .trim()
            .strip_prefix(&format!("{COOKIE_NAME}="))
            .expect("cookie is named fleet_web")
            .to_string()
    }

    /// End-to-end over a real socket: the point of this module is that an HTTP
    /// request reaches a `Backend` method and its return value comes back as
    /// JSON. The route table alone can't show that, so this drives the server
    /// with `NullBackend`, whose per-method returns happen to cover all four
    /// response shapes the dispatcher produces.
    #[test]
    fn http_requests_reach_the_backend() {
        let base = serve(TEST_PASSWORD);
        let cookie = login_cookie(&base, TEST_PASSWORD);
        let client = reqwest::blocking::Client::new();
        let get = |path: String| {
            client
                .get(path)
                .header("Cookie", format!("{COOKIE_NAME}={cookie}"))
                .send()
                .unwrap()
        };

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

    #[test]
    fn data_routes_require_a_session() {
        let base = serve(TEST_PASSWORD);
        let client = reqwest::blocking::Client::new();

        // No cookie at all.
        assert_eq!(
            client.get(format!("{base}/sessions")).send().unwrap().status(),
            401
        );

        // A cookie that was never issued.
        assert_eq!(
            client
                .get(format!("{base}/sessions"))
                .header("Cookie", format!("{COOKIE_NAME}=made-up"))
                .send()
                .unwrap()
                .status(),
            401
        );

        // Liveness stays open — it discloses nothing but "the door is up".
        assert_eq!(
            client.get(format!("{base}/health")).send().unwrap().status(),
            200
        );
    }

    #[test]
    fn wrong_password_is_rejected() {
        let base = serve(TEST_PASSWORD);
        let resp = reqwest::blocking::Client::new()
            .post(format!("{base}{LOGIN_PATH}"))
            .json(&serde_json::json!({ "password": "guess" }))
            .send()
            .unwrap();
        assert_eq!(resp.status(), 401);
        assert!(resp.headers().get("set-cookie").is_none());
    }

    /// A blank password must deny everything rather than admit anything — the
    /// invariant that makes a truncated/corrupt config fail closed.
    #[test]
    fn empty_password_admits_nobody() {
        let base = serve("");
        let client = reqwest::blocking::Client::new();
        for attempt in ["", "anything"] {
            let resp = client
                .post(format!("{base}{LOGIN_PATH}"))
                .json(&serde_json::json!({ "password": attempt }))
                .send()
                .unwrap();
            assert_eq!(resp.status(), 401, "empty config must reject {attempt:?}");
        }
    }

    #[test]
    fn logout_invalidates_the_session() {
        let base = serve(TEST_PASSWORD);
        let cookie = login_cookie(&base, TEST_PASSWORD);
        let client = reqwest::blocking::Client::new();
        let header = format!("{COOKIE_NAME}={cookie}");

        assert_eq!(
            client
                .get(format!("{base}/sessions"))
                .header("Cookie", &header)
                .send()
                .unwrap()
                .status(),
            200
        );

        client
            .post(format!("{base}{LOGOUT_PATH}"))
            .header("Cookie", &header)
            .send()
            .unwrap();

        assert_eq!(
            client
                .get(format!("{base}/sessions"))
                .header("Cookie", &header)
                .send()
                .unwrap()
                .status(),
            401,
            "the cookie must stop working after logout"
        );
    }

    #[test]
    fn disabled_config_refuses_to_bind() {
        let backend: SharedBackend = Arc::new(RwLock::new(Box::new(crate::NullBackend)));
        let result = start_with_config(
            backend,
            0,
            WebAccessConfig {
                enabled: false,
                password: TEST_PASSWORD.to_string(),
                bind_lan: false,
            },
            None,
        );
        assert!(result.is_err(), "disabled must not open a port");
    }

    /// A navigating browser must get the password form, not a bare 401 — that
    /// is the whole login UX, and it has to render before any bundle file is
    /// authorized.
    #[test]
    fn unauthorized_navigation_gets_the_login_page() {
        let base = serve_with_assets(TEST_PASSWORD, fake_bundle());
        let resp = reqwest::blocking::Client::new()
            .get(&base)
            .header("Accept", "text/html")
            .send()
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = resp.text().unwrap();
        assert!(body.contains("/web/login"), "login form must post to /web/login");
        assert!(
            !body.contains("board"),
            "the app bundle must not be served before login"
        );
    }

    /// The same unauthorized path via `fetch` (no `Accept: text/html`) must stay
    /// a 401, or the frontend would parse a login page as data.
    #[test]
    fn unauthorized_fetch_stays_a_401() {
        let base = serve_with_assets(TEST_PASSWORD, fake_bundle());
        let resp = reqwest::blocking::Client::new()
            .get(format!("{base}/sessions"))
            .header("Accept", "application/json")
            .send()
            .unwrap();
        assert_eq!(resp.status(), 401);
    }

    #[test]
    fn logged_in_browser_gets_the_bundle() {
        let base = serve_with_assets(TEST_PASSWORD, fake_bundle());
        let cookie = login_cookie(&base, TEST_PASSWORD);
        let client = reqwest::blocking::Client::new();
        let get = |path: String| {
            client
                .get(path)
                .header("Cookie", format!("{COOKIE_NAME}={cookie}"))
                .header("Accept", "text/html")
                .send()
                .unwrap()
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

    /// Data routes must win over the bundle: were the static arm to shadow one,
    /// the page would receive HTML where it expects JSON.
    #[test]
    fn data_routes_are_not_shadowed_by_static() {
        let base = serve_with_assets(TEST_PASSWORD, fake_bundle());
        let cookie = login_cookie(&base, TEST_PASSWORD);
        let resp = reqwest::blocking::Client::new()
            .get(format!("{base}/sessions"))
            .header("Cookie", format!("{COOKIE_NAME}={cookie}"))
            .header("Accept", "text/html")
            .send()
            .unwrap();
        assert_eq!(resp.status(), 200);
        assert_eq!(resp.text().unwrap(), "[]");
    }

    /// Browsers send every cookie for the origin in one header, and a segment
    /// with no `=` (a bare flag cookie) must not abort the scan before ours is
    /// reached. Driven over HTTP so it exercises the real `session_cookie`.
    #[test]
    fn session_is_found_among_other_cookies() {
        let base = serve(TEST_PASSWORD);
        let cookie = login_cookie(&base, TEST_PASSWORD);
        let resp = reqwest::blocking::Client::new()
            .get(format!("{base}/sessions"))
            .header(
                "Cookie",
                format!("theme=dark; flagged; {COOKIE_NAME}={cookie}; other=x"),
            )
            .send()
            .unwrap();
        assert_eq!(resp.status(), 200);
    }
}
