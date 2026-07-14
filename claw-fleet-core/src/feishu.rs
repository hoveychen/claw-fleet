//! Feishu (Lark) integration — shared types, env-credential probe, OAuth
//! callback listener, and the in-process bridge state.
//!
//! Architecture: both `LocalBackend` (desktop's in-process `Backend`) and
//! `fleet serve` (the remote probe HTTP daemon) drive the same store, so
//! the bridge lives here in `claw-fleet-core`.
//!
//! Coverage of the design at `design/feishu-integration.md`:
//!
//! - **Done:** types, env probe, PKCE S256, OAuth `state` (UUIDv4),
//!   localhost:51823 listener, code → token exchange, persistence
//!   (`~/.fleet/feishu.json`), disconnect, tenant_access_token cache,
//!   Card 2.0 send/update/urgent_app, decision_id → message_id map,
//!   webhook `/webhook/feishu` (`X-Lark-Signature` verification +
//!   `card.action.trigger` dispatch, see `handle_webhook`).
//!
//! Operator setup: see `design/feishu-integration.md#operator-setup-checklist`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Reserved loopback port for the OAuth redirect_uri.  Hard-coded so the
/// Feishu app's "Redirect URLs" allowlist is configured once.
pub const FEISHU_OAUTH_PORT: u16 = 51823;

/// TTL on a pending `state` token before we mark it `Failed`.
const OAUTH_TTL: Duration = Duration::from_secs(300);

/// HTTP timeout for the token + user_info exchange.
const FEISHU_HTTP_TIMEOUT: Duration = Duration::from_secs(15);

// ── Cross-process types ──────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct OauthHandle {
    pub state: String,
    pub authorize_url: String,
    pub port: u16,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OauthStatus {
    Pending,
    Connected {
        open_id: String,
        name: Option<String>,
    },
    Failed {
        reason: String,
    },
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum FeishuConnection {
    NotConnected,
    Connected {
        open_id: String,
        name: Option<String>,
    },
}

/// What kind of decision a card represents — drives whether `urgent_app`
/// fires and what subject text the resolved card shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionKind {
    Guard,
    Elicitation,
    PlanApproval,
}

impl DecisionKind {
    fn label(self) -> &'static str {
        match self {
            DecisionKind::Guard => "授权操作",
            DecisionKind::Elicitation => "Decision needed",
            DecisionKind::PlanApproval => "Plan review",
        }
    }
}

// ── App credentials (file-first, env fallback) ───────────────────────────────

pub struct AppCredentials {
    pub app_id: String,
    pub app_secret: String,
    pub encrypt_key: Option<String>,
    pub verification_token: Option<String>,
}

/// Plain payload for the UI / cross-process getters.  Empty strings encode
/// "not set" (rather than `Option`) so the React form can bind directly.
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub struct StoredCreds {
    pub app_id: String,
    pub app_secret: String,
    pub encrypt_key: String,
    pub verification_token: String,
}

impl AppCredentials {
    pub fn from_env() -> Result<Self, String> {
        let app_id = std::env::var("FEISHU_APP_ID")
            .map_err(|_| "FEISHU_APP_ID not set".to_string())?;
        let app_secret = std::env::var("FEISHU_APP_SECRET")
            .map_err(|_| "FEISHU_APP_SECRET not set".to_string())?;
        Ok(AppCredentials {
            app_id,
            app_secret,
            encrypt_key: std::env::var("FEISHU_ENCRYPT_KEY").ok(),
            verification_token: std::env::var("FEISHU_VERIFICATION_TOKEN").ok(),
        })
    }

    /// Production loader: prefer `~/.fleet/feishu-creds.json`, fall back to
    /// env vars. The UI writes the file so users don't need shell-level
    /// `export` (which macOS Finder-launched apps never see).
    pub fn load() -> Result<Self, String> {
        if let Some(file) = load_creds_file() {
            if !file.app_id.is_empty() && !file.app_secret.is_empty() {
                return Ok(AppCredentials {
                    app_id: file.app_id,
                    app_secret: file.app_secret,
                    encrypt_key: if file.encrypt_key.is_empty() { None } else { Some(file.encrypt_key) },
                    verification_token: if file.verification_token.is_empty() { None } else { Some(file.verification_token) },
                });
            }
        }
        Self::from_env()
    }
}

/// UI getter: return the persisted credentials (empty struct if none stored).
pub fn get_stored_creds() -> StoredCreds {
    load_creds_file().unwrap_or_default()
}

/// UI setter: persist the credentials, or clear the file when all fields are empty.
pub fn set_stored_creds(c: StoredCreds) -> Result<(), String> {
    if c.app_id.is_empty() && c.app_secret.is_empty()
        && c.encrypt_key.is_empty() && c.verification_token.is_empty()
    {
        delete_creds_file();
        return Ok(());
    }
    if c.app_id.is_empty() || c.app_secret.is_empty() {
        return Err("app_id and app_secret are both required".into());
    }
    save_creds_file(&c)?;
    // Fresh/updated creds → (re)start the outbound WS long-connection that
    // receives card.action.trigger callbacks (idempotent; no-op without creds).
    ensure_ws_client();
    Ok(())
}

// ── In-process bridge state ──────────────────────────────────────────────────

#[derive(Clone, Debug)]
struct PendingOauth {
    code_verifier: String,
    created_at: Instant,
    status: OauthStatus,
}

#[derive(Clone, Debug)]
struct BoundIdentity {
    open_id: String,
    name: Option<String>,
}

#[derive(Clone, Debug)]
struct CachedTenantToken {
    token: String,
    /// Wall-clock instant after which we must refresh.
    expires_at: Instant,
}

/// Lifecycle of a single Feishu card relative to the desktop response.
///
/// `Sending` covers the window where `send_card`'s HTTP POST is in
/// flight.  If the desktop user resolves the decision before the POST
/// returns, the state flips to `ResolvedBeforeSent` and the send-thread
/// applies the read-only update as soon as it has a `message_id`.
#[derive(Clone, Debug)]
enum DecisionState {
    Sending,
    Sent { message_id: String },
    /// Resolved while the send was still in flight; carries the fully
    /// rendered "已处理" card the send-thread applies once it has a
    /// `message_id`.
    ResolvedBeforeSent { card: serde_json::Value },
}

struct Bridge {
    pending: Mutex<HashMap<String, PendingOauth>>,
    connection: Mutex<Option<BoundIdentity>>,
    tenant_token: Mutex<Option<CachedTenantToken>>,
    /// Maps `decision_id` → its current Feishu lifecycle state.
    decision_states: Mutex<HashMap<String, DecisionState>>,
}

fn bridge() -> &'static Bridge {
    static B: OnceLock<Bridge> = OnceLock::new();
    B.get_or_init(|| Bridge {
        pending: Mutex::new(HashMap::new()),
        connection: Mutex::new(load_persistence()),
        tenant_token: Mutex::new(None),
        decision_states: Mutex::new(HashMap::new()),
    })
}

/// Returns the bound user's `open_id`, or None if not connected.
fn bound_open_id() -> Option<String> {
    bridge()
        .connection
        .lock()
        .unwrap()
        .as_ref()
        .map(|b| b.open_id.clone())
}

// ── Public entry points ──────────────────────────────────────────────────────

/// Validates env, ensures the localhost callback listener is running, mints
/// fresh `state` + PKCE pair, registers them, and returns the handle the
/// desktop opens in the user's browser.
pub fn start_oauth() -> Result<OauthHandle, String> {
    let creds = AppCredentials::load()?;
    ensure_listener()?;

    let state = mint_state();
    let code_verifier = mint_code_verifier();
    let code_challenge = pkce_s256(&code_verifier);

    bridge().pending.lock().unwrap().insert(
        state.clone(),
        PendingOauth {
            code_verifier,
            created_at: Instant::now(),
            status: OauthStatus::Pending,
        },
    );

    let redirect = format!("http://localhost:{}/feishu/cb", FEISHU_OAUTH_PORT);
    let authorize_url = format!(
        "https://accounts.feishu.cn/open-apis/authen/v1/authorize\
         ?app_id={app_id}\
         &redirect_uri={redirect}\
         &state={state}\
         &code_challenge={code_challenge}\
         &code_challenge_method=S256\
         &scope={scope}",
        app_id = creds.app_id,
        redirect = url_encode(&redirect),
        state = state,
        code_challenge = code_challenge,
        scope = url_encode("im:message im:message.urgent im:resource"),
    );

    Ok(OauthHandle { state, authorize_url, port: FEISHU_OAUTH_PORT })
}

/// Reports the current status for `state`, marking it `Failed` if it
/// exceeded the TTL.
pub fn poll_oauth(state: &str) -> Result<OauthStatus, String> {
    let mut pending = bridge().pending.lock().unwrap();
    let Some(entry) = pending.get_mut(state) else {
        return Err("unknown oauth state (expired or never started)".into());
    };
    if matches!(entry.status, OauthStatus::Pending)
        && entry.created_at.elapsed() > OAUTH_TTL
    {
        entry.status = OauthStatus::Failed { reason: "oauth flow timed out".into() };
    }
    Ok(entry.status.clone())
}

pub fn status() -> Result<FeishuConnection, String> {
    let conn = bridge().connection.lock().unwrap();
    Ok(match &*conn {
        Some(b) => FeishuConnection::Connected {
            open_id: b.open_id.clone(),
            name: b.name.clone(),
        },
        None => FeishuConnection::NotConnected,
    })
}

pub fn disconnect() -> Result<(), String> {
    *bridge().connection.lock().unwrap() = None;
    *bridge().tenant_token.lock().unwrap() = None;
    bridge().decision_states.lock().unwrap().clear();
    delete_persistence();
    Ok(())
}

// ── OAuth callback listener ─────────────────────────────────────────────────

/// Lazy-spawn the `127.0.0.1:51823` listener thread on first
/// `start_oauth`.  Idempotent — subsequent calls are no-ops.  Returns an
/// error if the bind fails (port busy or otherwise).
fn ensure_listener() -> Result<(), String> {
    static STATE: OnceLock<Mutex<bool>> = OnceLock::new();
    let mu = STATE.get_or_init(|| Mutex::new(false));
    let mut started = mu.lock().unwrap();
    if *started {
        return Ok(());
    }
    let server = tiny_http::Server::http(format!("127.0.0.1:{}", FEISHU_OAUTH_PORT))
        .map_err(|e| format!("failed to bind feishu oauth listener on :{}: {}", FEISHU_OAUTH_PORT, e))?;
    *started = true;

    std::thread::Builder::new()
        .name("feishu-oauth-listener".into())
        .spawn(move || {
            for request in server.incoming_requests() {
                dispatch_listener(request);
            }
        })
        .map_err(|e| format!("failed to spawn feishu oauth listener thread: {e}"))?;
    Ok(())
}

/// Path dispatcher for the loopback listener.  The same socket serves
/// the OAuth callback (browser GET) and the Feishu event webhook (POST
/// from Lark's servers, reachable only when the operator tunnels the
/// port to the public internet — see
/// `design/feishu-integration.md#operator-setup-checklist`).
fn dispatch_listener(request: tiny_http::Request) {
    let url = request.url().to_string();
    let (path, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (url, String::new()),
    };

    match path.as_str() {
        "/feishu/cb" => handle_oauth_callback(request, &query),
        "/webhook/feishu" => handle_webhook_request(request),
        _ => {
            let _ = request.respond(tiny_http::Response::empty(404));
        }
    }
}

/// Handle the OAuth browser callback.  `query` is the raw query string
/// (already split off from the path).
fn handle_oauth_callback(request: tiny_http::Request, query: &str) {
    let qs = parse_query(query);
    let state = qs.get("state").cloned().unwrap_or_default();
    let code = qs.get("code").cloned().unwrap_or_default();

    let outcome = exchange_and_bind(&state, &code);
    let html = match &outcome {
        Ok(()) => "<!doctype html><html><head><meta charset=\"utf-8\"><title>Fleet · Feishu connected</title></head><body style=\"font-family:system-ui;text-align:center;padding:48px\"><h2>已连接到 Fleet</h2><p>可以关闭这个标签页了。</p></body></html>".to_string(),
        Err(e) => format!("<!doctype html><html><head><meta charset=\"utf-8\"><title>Fleet · Feishu OAuth failed</title></head><body style=\"font-family:system-ui;text-align:center;padding:48px\"><h2>授权失败</h2><pre style=\"text-align:left;display:inline-block\">{}</pre></body></html>",
            html_escape(e)),
    };

    let header: tiny_http::Header = "Content-Type: text/html; charset=utf-8".parse().unwrap();
    let _ = request.respond(tiny_http::Response::from_string(html).with_header(header));
}

/// Read the inbound webhook body, hand it to [`handle_webhook`], and
/// reply with the resulting JSON (or a 4xx/5xx on validation failure).
fn handle_webhook_request(mut request: tiny_http::Request) {
    let timestamp = header_value(&request, "X-Lark-Request-Timestamp");
    let nonce = header_value(&request, "X-Lark-Request-Nonce");
    let signature = header_value(&request, "X-Lark-Signature");

    let mut body = Vec::new();
    if let Err(e) = std::io::Read::read_to_end(request.as_reader(), &mut body) {
        let header: tiny_http::Header = "Content-Type: application/json".parse().unwrap();
        let resp = serde_json::json!({"error": format!("read body: {e}")}).to_string();
        let _ = request.respond(
            tiny_http::Response::from_string(resp)
                .with_status_code(400)
                .with_header(header),
        );
        return;
    }

    let (status, response_body) = match handle_webhook(
        &body,
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
    let header: tiny_http::Header = "Content-Type: application/json".parse().unwrap();
    let _ = request.respond(
        tiny_http::Response::from_data(response_body)
            .with_status_code(status)
            .with_header(header),
    );
}

fn header_value(request: &tiny_http::Request, name: &str) -> Option<String> {
    request
        .headers()
        .iter()
        .find(|h| h.field.as_str().as_str().eq_ignore_ascii_case(name))
        .map(|h| h.value.as_str().to_string())
}

/// Exchange `code + code_verifier` for `access_token`, fetch `open_id +
/// name`, persist the binding, and update the pending entry.
///
/// On any failure, marks the pending entry as `Failed` so the desktop
/// poll surfaces the reason — but still returns the same `Err` for the
/// HTML response.
fn exchange_and_bind(state: &str, code: &str) -> Result<(), String> {
    let result = exchange_and_bind_inner(state, code);
    if let Err(ref e) = result {
        let mut pending = bridge().pending.lock().unwrap();
        if let Some(entry) = pending.get_mut(state) {
            entry.status = OauthStatus::Failed { reason: e.clone() };
        }
    }
    result
}

fn exchange_and_bind_inner(state: &str, code: &str) -> Result<(), String> {
    if state.is_empty() || code.is_empty() {
        return Err("missing `state` or `code` query parameter".into());
    }
    let creds = AppCredentials::load()?;

    let code_verifier = {
        let pending = bridge().pending.lock().unwrap();
        pending
            .get(state)
            .ok_or_else(|| "unknown oauth state".to_string())
            .map(|e| e.code_verifier.clone())?
    };

    let client = http_client()?;

    let redirect = format!("http://localhost:{}/feishu/cb", FEISHU_OAUTH_PORT);

    // Token exchange — Feishu's v2 endpoint accepts client_secret_post + PKCE.
    let token_payload: serde_json::Value = client
        .post("https://open.feishu.cn/open-apis/authen/v2/oauth/token")
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code),
            ("code_verifier", &code_verifier),
            ("client_id", &creds.app_id),
            ("client_secret", &creds.app_secret),
            ("redirect_uri", &redirect),
        ])
        .send()
        .map_err(|e| format!("token exchange: {e}"))?
        .json()
        .map_err(|e| format!("token exchange parse: {e}"))?;

    // v2 returns flat fields; tolerate the v1-style `data` envelope just
    // in case the operator wires a different endpoint.
    let access_token = token_payload
        .get("access_token")
        .or_else(|| token_payload.get("data").and_then(|d| d.get("access_token")))
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("token response missing access_token: {token_payload}"))?
        .to_string();

    // user_info — extract open_id and display name, then drop the token.
    let user_payload: serde_json::Value = client
        .get("https://open.feishu.cn/open-apis/authen/v1/user_info")
        .bearer_auth(&access_token)
        .send()
        .map_err(|e| format!("user_info: {e}"))?
        .json()
        .map_err(|e| format!("user_info parse: {e}"))?;

    let user_data = user_payload
        .get("data")
        .unwrap_or(&user_payload);
    let open_id = user_data
        .get("open_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("user_info missing open_id: {user_payload}"))?
        .to_string();
    let name = user_data
        .get("name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Update pending entry → Connected.
    {
        let mut pending = bridge().pending.lock().unwrap();
        if let Some(entry) = pending.get_mut(state) {
            entry.status = OauthStatus::Connected {
                open_id: open_id.clone(),
                name: name.clone(),
            };
        }
    }
    // Persist the binding.
    {
        let mut conn = bridge().connection.lock().unwrap();
        *conn = Some(BoundIdentity {
            open_id: open_id.clone(),
            name: name.clone(),
        });
    }
    let _ = save_persistence(&BoundIdentity { open_id, name });
    Ok(())
}

// ── Webhook handling ─────────────────────────────────────────────────────────

/// Handle an inbound `/webhook/feishu` POST.  Returns the JSON body the
/// server should reply with on success, or an error string for 4xx.
///
/// Coverage:
/// - URL verification challenge (initial handshake) — required for the
///   Feishu console to accept the URL.
/// - V2 `card.action.trigger` events — buttons clicked on a card.
/// - Signature validation via `X-Lark-Signature` *only* when the operator
///   set `FEISHU_ENCRYPT_KEY`; otherwise we fall back to the `token`
///   field embedded in the body matching `FEISHU_VERIFICATION_TOKEN`.
/// - Encrypted bodies (`{"encrypt":"..."}`) are rejected with 4xx —
///   operators should leave encryption *off* in the Feishu console.
pub fn handle_webhook(
    raw_body: &[u8],
    timestamp: Option<&str>,
    nonce: Option<&str>,
    signature: Option<&str>,
) -> Result<Vec<u8>, String> {
    let creds = AppCredentials::load()?;
    let body_str = std::str::from_utf8(raw_body).map_err(|e| format!("body not utf-8: {e}"))?;
    let body: serde_json::Value =
        serde_json::from_str(body_str).map_err(|e| format!("body not json: {e}"))?;

    if body.get("encrypt").is_some() {
        return Err(
            "encrypted webhook body — disable encryption in the Feishu app's event subscription"
                .into(),
        );
    }

    // Signature check (only meaningful when encrypt_key is set).
    if let (Some(ts), Some(nc), Some(sig), Some(key)) =
        (timestamp, nonce, signature, &creds.encrypt_key)
    {
        let expected = sha256_hex(&format!("{ts}{nc}{key}{body_str}"));
        if !constant_time_eq(expected.as_bytes(), sig.as_bytes()) {
            return Err("invalid X-Lark-Signature".into());
        }
    }

    // V1 URL verification challenge.
    if body.get("type").and_then(|v| v.as_str()) == Some("url_verification") {
        if let Some(expected) = &creds.verification_token {
            let token = body.get("token").and_then(|v| v.as_str()).unwrap_or("");
            if !constant_time_eq(expected.as_bytes(), token.as_bytes()) {
                return Err("invalid verification token".into());
            }
        }
        let challenge = body
            .get("challenge")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        return Ok(serde_json::json!({"challenge": challenge})
            .to_string()
            .into_bytes());
    }

    // V2 events all live under `header.event_type`.
    if let Some(expected) = &creds.verification_token {
        let token = body
            .pointer("/header/token")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if !constant_time_eq(expected.as_bytes(), token.as_bytes()) {
            return Err("invalid verification token".into());
        }
    }

    let event_type = body
        .pointer("/header/event_type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    match event_type {
        "card.action.trigger" => handle_card_action(&body),
        _ => Ok(b"{}".to_vec()),
    }
}

// ── WebSocket long-connection (card.action.trigger over an outbound WS) ──────
//
// Replaces the public-internet webhook for receiving card button clicks: the
// process opens an *outbound* WebSocket to Feishu (no inbound port, no tunnel,
// works behind a firewall) and `card.action.trigger` events arrive over it.
// Topology = scheme A — each desktop holds its own app credentials and opens
// its own connection.  The received payload is the same JSON envelope the
// webhook used to receive, so it is routed straight into `handle_card_action`.

use std::sync::atomic::{AtomicBool, Ordering};

static WS_STARTED: AtomicBool = AtomicBool::new(false);

/// Ensure the outbound Feishu WebSocket long-connection is running.
///
/// Idempotent: the first call with valid credentials spawns one background
/// thread (with its own tokio runtime) that connects and auto-reconnects.
/// No-op when credentials are absent — safe to call again after creds are set.
pub fn ensure_ws_client() {
    let has_creds = AppCredentials::load()
        .map(|c| !c.app_id.is_empty() && !c.app_secret.is_empty())
        .unwrap_or(false);
    if !has_creds {
        return;
    }
    if WS_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("feishu-ws".into())
        .spawn(|| {
            match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt.block_on(ws_run_loop()),
                Err(e) => {
                    eprintln!("feishu ws: runtime build failed: {e}");
                    WS_STARTED.store(false, Ordering::SeqCst);
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("feishu ws: spawn failed: {e}");
        WS_STARTED.store(false, Ordering::SeqCst);
    }
}

/// Reconnect loop: `LarkWsClient::open` resolves when the socket drops, so we
/// reconnect after a short backoff.  Bails out only if credentials disappear.
async fn ws_run_loop() {
    loop {
        match ws_connect_once().await {
            Ok(()) => {
                crate::log_debug("[feishu ws] disconnected; reconnecting in 5s");
            }
            Err(e) => {
                crate::log_debug(&format!("[feishu ws] error: {e}"));
                if e == "no-creds" {
                    WS_STARTED.store(false, Ordering::SeqCst);
                    return;
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

// ── Hand-written larkws client ───────────────────────────────────────────────
//
// A minimal port of open-lark 0.16.1's `ws_client` (client.rs + frame_handler.rs),
// keeping only the `card.action.trigger` path.  We dropped the `open_lark` crate
// (bad gitlink); the wire protocol is reproduced here directly against
// `tokio-tungstenite` + the `lark-websocket-protobuf` `Frame`/`Header` types.
//
// Flow:
//   1. POST app_id/app_secret to `…/callback/ws/endpoint` → ws URL (query carries
//      `service_id`) + `ClientConfig` (ping_interval seconds).
//   2. `connect_async(url)`, split into sink/stream.
//   3. select! loop:
//        • inbound Binary → decode `Frame`; control(0) is pong/ignored, data(1)
//          is reassembled (sum/seq/message_id fragmenting) then routed into
//          `dispatch_ws_payload`, and we send back an ack response frame.
//        • ping interval tick → send a control ping frame.
//        • 1s checkout tick → reconnect if >120s silent.

use lark_websocket_protobuf::pbbp2::{Frame, Header};
use prost::Message as _;

const WS_ENDPOINT_URL: &str = "https://open.feishu.cn/callback/ws/endpoint";
/// Reconnect if no inbound traffic for this long (mirrors open-lark).
const WS_HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(120);

#[derive(Deserialize)]
struct WsEndpointEnvelope {
    #[serde(default)]
    code: i32,
    #[serde(default)]
    msg: String,
    data: Option<WsEndpointData>,
}

#[derive(Deserialize)]
struct WsEndpointData {
    #[serde(rename = "URL")]
    url: Option<String>,
    #[serde(rename = "ClientConfig")]
    client_config: Option<WsClientConfig>,
}

#[derive(Deserialize, Clone)]
struct WsClientConfig {
    /// Ping heartbeat interval, seconds.
    #[serde(rename = "PingInterval", default)]
    ping_interval: i32,
}

/// Resolved connection target: the ws URL, its `service_id`, and the ping period.
struct WsEndpoint {
    url: String,
    service_id: i32,
    ping_interval: Duration,
}

/// Fetch the outbound ws endpoint from Feishu using the app credentials.
///
/// Uses the async `reqwest::Client` (the rest of `feishu.rs` is blocking, but
/// this call lives inside the ws tokio runtime, so it must not block).
async fn ws_get_endpoint(app_id: &str, app_secret: &str) -> Result<WsEndpoint, String> {
    let client = reqwest::Client::builder()
        .timeout(FEISHU_HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("ws endpoint client init: {e}"))?;
    let resp: WsEndpointEnvelope = client
        .post(WS_ENDPOINT_URL)
        .header("locale", "zh")
        // Official BootstrapRequest sends all three fields (ClientAssertion is
        // empty when authenticating with AppSecret); mirror it for parity.
        .json(&serde_json::json!({ "AppID": app_id, "AppSecret": app_secret, "ClientAssertion": "" }))
        .send()
        .await
        .map_err(|e| format!("ws endpoint request: {e}"))?
        .json()
        .await
        .map_err(|e| format!("ws endpoint parse: {e}"))?;

    if resp.code != 0 {
        return Err(format!("ws endpoint error code={} msg={}", resp.code, resp.msg));
    }
    let data = resp.data.ok_or_else(|| "ws endpoint missing data".to_string())?;
    let url = data
        .url
        .filter(|u| !u.is_empty())
        .ok_or_else(|| "ws endpoint missing URL".to_string())?;
    let service_id = parse_service_id(&url)
        .ok_or_else(|| format!("ws endpoint URL missing service_id: {url}"))?;
    let ping_secs = data
        .client_config
        .map(|c| c.ping_interval)
        .filter(|s| *s > 0)
        .unwrap_or(120) as u64;
    Ok(WsEndpoint {
        url,
        service_id,
        ping_interval: Duration::from_secs(ping_secs),
    })
}

/// Pull `service_id` out of the ws URL's query string.  Hand-rolled so we don't
/// pull in the `url` crate just for one query param.
fn parse_service_id(url: &str) -> Option<i32> {
    let query = url.split_once('?').map(|(_, q)| q).unwrap_or("");
    for pair in query.split('&') {
        if let Some((k, v)) = pair.split_once('=') {
            if k == "service_id" {
                return v.parse().ok();
            }
        }
    }
    None
}

fn frame_header_value<'a>(headers: &'a [Header], key: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|h| h.key == key)
        .map(|h| h.value.as_str())
}

/// Per-`message_id` fragment buffer for data frames split across multiple ws
/// frames (`sum`/`seq` headers).  Ported from open-lark's `FramePackageBuffer`.
#[derive(Default)]
struct FragmentBuffer {
    sum: usize,
    parts: Vec<Option<Vec<u8>>>,
    received: usize,
}

impl FragmentBuffer {
    fn new(sum: usize) -> Self {
        Self {
            sum,
            parts: vec![None; sum],
            received: 0,
        }
    }

    fn insert(&mut self, seq: usize, payload: Vec<u8>) {
        if seq >= self.sum {
            return;
        }
        if self.parts[seq].is_none() {
            self.received += 1;
        }
        self.parts[seq] = Some(payload);
    }

    fn is_complete(&self) -> bool {
        self.sum > 0 && self.received == self.sum && self.parts.iter().all(|p| p.is_some())
    }

    fn combine(self) -> Vec<u8> {
        let mut out = Vec::new();
        for part in self.parts.into_iter().flatten() {
            out.extend_from_slice(&part);
        }
        out
    }
}

/// Build a control ping frame (method=0, single `type: ping` header).
/// Mirrors `FrameHandler::build_ping_frame`.
fn build_ping_frame(service_id: i32) -> Frame {
    Frame {
        seq_id: 0,
        log_id: 0,
        service: service_id,
        method: 0, // control
        headers: vec![Header {
            key: "type".to_string(),
            value: "ping".to_string(),
        }],
        payload_encoding: None,
        payload_type: None,
        payload: None,
        log_id_new: None,
    }
}

/// Build the ack frame Feishu expects after a data event: echo the inbound data
/// frame's headers, append a `biz_rt` header, and set the payload to the
/// `{"code":200,...}` response envelope (the `NewWsResponse` shape open-lark
/// serialises).  Mirrors `FrameHandler::handle_data_frame`'s `"event"` branch.
fn build_ack_frame(received: &Frame, biz_rt_ms: u128) -> Frame {
    let mut headers = received.headers.clone();
    headers.push(Header {
        key: "biz_rt".to_string(),
        value: biz_rt_ms.to_string(),
    });
    let response = serde_json::json!({
        "code": 200u16,
        "headers": { "biz_rt": biz_rt_ms.to_string() },
        "data": Vec::<u8>::new(),
    });
    let payload = serde_json::to_vec(&response).unwrap_or_default();
    Frame {
        seq_id: received.seq_id,
        log_id: received.log_id,
        service: received.service,
        method: received.method, // data
        headers,
        payload_encoding: received.payload_encoding.clone(),
        payload_type: received.payload_type.clone(),
        payload: Some(payload),
        log_id_new: received.log_id_new.clone(),
    }
}

/// Reassemble a data frame's payload, returning `Some(bytes)` once a full
/// message is available (single-frame messages return immediately; fragments
/// return `None` until the last piece arrives).  Ported from open-lark's
/// `process_frame_packages_internal`.
fn reassemble<'a>(
    buffers: &mut HashMap<String, FragmentBuffer>,
    headers: &[Header],
    payload: Vec<u8>,
) -> Option<Vec<u8>> {
    let sum = frame_header_value(headers, "sum")
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|s| *s > 0)
        .unwrap_or(1);
    let seq = frame_header_value(headers, "seq")
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(0);
    let msg_id = frame_header_value(headers, "message_id").unwrap_or("");

    // Single frame, or un-aggregatable fragment → pass straight through.
    if sum == 1 || msg_id.is_empty() || seq >= sum {
        return Some(payload);
    }

    let buffer = buffers
        .entry(msg_id.to_string())
        .or_insert_with(|| FragmentBuffer::new(sum));
    if buffer.sum != sum {
        *buffer = FragmentBuffer::new(sum);
    }
    buffer.insert(seq, payload);
    if !buffer.is_complete() {
        return None;
    }
    buffers.remove(msg_id).map(FragmentBuffer::combine)
}

async fn ws_connect_once() -> Result<(), String> {
    let creds = AppCredentials::load().map_err(|_| "no-creds".to_string())?;
    if creds.app_id.is_empty() || creds.app_secret.is_empty() {
        return Err("no-creds".to_string());
    }

    let endpoint = ws_get_endpoint(&creds.app_id, &creds.app_secret).await?;
    let service_id = endpoint.service_id;

    let (stream, _resp) = tokio_tungstenite::connect_async(&endpoint.url)
        .await
        .map_err(|e| format!("ws connect: {e}"))?;
    crate::log_debug(&format!(
        "[feishu ws] connected: service_id={service_id} ping_interval={:?}",
        endpoint.ping_interval
    ));

    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let (mut sink, mut source) = stream.split();
    let mut buffers: HashMap<String, FragmentBuffer> = HashMap::new();

    let mut ping_timer = tokio::time::interval(endpoint.ping_interval);
    // The first tick fires immediately; skip it so we don't ping before the
    // server is ready.
    ping_timer.tick().await;
    let mut checkout = tokio::time::interval(Duration::from_secs(1));
    let mut last_seen = tokio::time::Instant::now();
    // Reconnect only after ~3 missed pings, not 1 — a long idle wait (user
    // takes 30+ min to tap a card) must not trigger spurious reconnects. The
    // connection stays alive on ping/pong regardless of how long the wait is.
    let heartbeat_timeout = endpoint
        .ping_interval
        .saturating_mul(3)
        .max(WS_HEARTBEAT_TIMEOUT);

    loop {
        tokio::select! {
            item = source.next() => {
                match item {
                    Some(Ok(msg)) => {
                        last_seen = tokio::time::Instant::now();
                        if let Some(ack) = handle_ws_message(msg, service_id, &mut buffers) {
                            let bytes = ack.encode_to_vec();
                            sink.send(Message::Binary(bytes.into()))
                                .await
                                .map_err(|e| format!("ws send ack: {e}"))?;
                        }
                    }
                    Some(Err(e)) => return Err(format!("ws recv: {e}")),
                    None => return Ok(()), // stream closed ⇒ reconnect
                }
            }
            _ = ping_timer.tick() => {
                let frame = build_ping_frame(service_id);
                let bytes = frame.encode_to_vec();
                sink.send(Message::Binary(bytes.into()))
                    .await
                    .map_err(|e| format!("ws send ping: {e}"))?;
            }
            _ = checkout.tick() => {
                if last_seen.elapsed() > heartbeat_timeout {
                    return Err("ws heartbeat timeout".to_string());
                }
            }
        }
    }
}

/// Handle one inbound ws message.  Returns `Some(frame)` when an ack should be
/// sent back (only for fully-reassembled `event` data frames).  Pings, pongs,
/// control frames, `card` frames, and mid-fragment data frames return `None`.
fn handle_ws_message(
    msg: tokio_tungstenite::tungstenite::Message,
    _service_id: i32,
    buffers: &mut HashMap<String, FragmentBuffer>,
) -> Option<Frame> {
    use tokio_tungstenite::tungstenite::Message;
    let data = match msg {
        Message::Binary(data) => data,
        // Ping/Pong/Close/Text carry no Lark frame; tungstenite auto-replies to
        // protocol-level pings, so we just refresh liveness (done by caller).
        _ => return None,
    };

    let frame = match Frame::decode(&*data) {
        Ok(f) => f,
        Err(e) => {
            crate::log_debug(&format!("[feishu ws] frame decode error: {e}"));
            return None;
        }
    };

    match frame.method {
        // control: pong / server keepalive — nothing to ack.
        0 => None,
        // data
        1 => {
            crate::log_debug(&format!(
                "[feishu ws] data frame: {} bytes, type={:?}",
                data.len(),
                frame_header_value(&frame.headers, "type")
            ));
            handle_data_frame(frame, buffers)
        }
        other => {
            crate::log_debug(&format!("[feishu ws] unknown frame method {other}"));
            None
        }
    }
}

/// Process a data frame: reassemble fragments, dispatch the `event` payload into
/// `dispatch_ws_payload`, and produce the ack frame.  `card` frames are skipped
/// (card.action.trigger does NOT arrive as a `card` frame — it's an `event`).
fn handle_data_frame(
    mut frame: Frame,
    buffers: &mut HashMap<String, FragmentBuffer>,
) -> Option<Frame> {
    let msg_type = frame_header_value(&frame.headers, "type")
        .unwrap_or("")
        .to_string();

    match msg_type.as_str() {
        "event" | "" => {
            let payload = frame.payload.take()?;
            let full = reassemble(buffers, &frame.headers, payload)?;
            // Ack the transport IMMEDIATELY and run the business logic on a
            // plain OS thread. Two reasons, both fatal to the ack otherwise:
            //   1. Feishu fails a card.action.trigger callback with "target
            //      callback service has timed out and there is no response"
            //      if it doesn't get the ack within ~3s, and the dispatch
            //      path does a blocking `update_card` HTTP round-trip.
            //   2. `update_card` uses `reqwest::blocking`, which PANICS when
            //      called from within a tokio runtime thread ("Cannot block
            //      the current thread from within a runtime"). The WS loop IS
            //      such a thread, so running dispatch inline killed the WS
            //      thread before the ack could be sent.
            // A std::thread is not a tokio runtime thread, so the blocking
            // client is happy there. The ack carries no business payload (the
            // card is updated out-of-band), so it need not wait for dispatch.
            std::thread::spawn(move || dispatch_ws_payload(&full));
            Some(build_ack_frame(&frame, 0))
        }
        "card" => None,
        _ => None,
    }
}

/// Route a raw WS event payload (the same JSON envelope the webhook received)
/// to the existing card-action handler.  Only `card.action.trigger` is acted
/// on; all other event types are ignored.
fn dispatch_ws_payload(payload: &[u8]) {
    // Log the raw payload verbatim before parsing — this is the one shape we
    // could never confirm against a live Feishu callback, so capturing the
    // exact JSON is the whole point of this instrumentation.
    crate::log_debug(&format!(
        "[feishu ws] event payload: {}",
        String::from_utf8_lossy(payload)
    ));
    let body: serde_json::Value = match serde_json::from_slice(payload) {
        Ok(v) => v,
        Err(e) => {
            crate::log_debug(&format!("[feishu ws] payload not json: {e}"));
            return;
        }
    };
    let event_type = body
        .pointer("/header/event_type")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    crate::log_debug(&format!("[feishu ws] event_type={event_type}"));
    if event_type == "card.action.trigger" {
        match handle_card_action(&body) {
            Ok(_) => crate::log_debug("[feishu ws] card action handled OK"),
            Err(e) => crate::log_debug(&format!("[feishu ws] card action ERROR: {e}")),
        }
    }
}

fn handle_card_action(body: &serde_json::Value) -> Result<Vec<u8>, String> {
    let value = body
        .pointer("/event/action/value")
        .ok_or_else(|| "missing event.action.value".to_string())?;
    let action = value
        .get("action")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing action".to_string())?;
    let decision_id = value
        .get("decision_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "missing decision_id".to_string())?;
    crate::log_debug(&format!(
        "[feishu ws] card action={action} decision_id={decision_id} value={value}"
    ));

    match action {
        "guard" => {
            let allow = value.get("allow").and_then(|v| v.as_bool()).unwrap_or(false);
            let resp = crate::guard::GuardResponse {
                id: decision_id.to_string(),
                decision: if allow {
                    crate::guard::GuardDecision::Allow
                } else {
                    crate::guard::GuardDecision::Block
                },
                reason: None,
            };
            crate::guard::write_response(&resp)?;
            let summary = if allow { "✅ Allow (Feishu)" } else { "🚫 Block (Feishu)" };
            let _ = notify_decision_resolved(decision_id, resolved_card(DecisionKind::Guard, summary));
        }
        "elicitation" => {
            let answers = if let Some(fields) = value.get("fields") {
                // New single-card form: the Submit button carries a
                // `fields` map (q0 → question text); the submitted values
                // live in the form-submit payload.
                let form = extract_form_value(body);
                crate::log_debug(&format!(
                    "[feishu ws] elicitation fields={fields} extracted_form={form:?}"
                ));
                assemble_elicitation_answers(fields, form)
            } else {
                // Backwards-compat: a pre-upgrade card still in flight
                // carried a single answer_label/question pair.
                let mut a = HashMap::new();
                let q = value.get("question").and_then(|v| v.as_str()).unwrap_or("");
                let label = value
                    .get("answer_label")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if !q.is_empty() {
                    a.insert(q.to_string(), label.to_string());
                }
                a
            };
            // No question answered → treat the submit as a decline so the
            // agent doesn't proceed on empty input.
            let declined = answers.is_empty();
            crate::log_debug(&format!(
                "[feishu ws] elicitation answers={answers:?} declined={declined}"
            ));
            // Render the read-only card (questions + all options, pick(s)
            // highlighted) before `answers` is moved into the response —
            // resolved_elicitation_card reads the still-present request file
            // for the option lists.
            let resolved = resolved_elicitation_card(decision_id, &answers, declined);
            let resp = crate::elicitation::ElicitationResponse {
                id: decision_id.to_string(),
                declined,
                answers,
            };
            crate::parked::deliver(decision_id, &resp, declined, crate::elicitation::write_response)?;
            let _ = notify_decision_resolved(decision_id, resolved);
        }
        "plan" => {
            let decision = value
                .get("decision")
                .and_then(|v| v.as_str())
                .unwrap_or("reject");
            let resp = crate::plan_approval::PlanApprovalResponse {
                id: decision_id.to_string(),
                decision: decision.to_string(),
                edited_plan: None,
                feedback: None,
            };
            // `dismissed: false` — a rejection is an answer, and a parked card's
            // agent has to be woken up to hear it.
            crate::parked::deliver(decision_id, &resp, false, crate::plan_approval::write_response)?;
            let summary = if decision == "approve" {
                "✅ Approved (Feishu)"
            } else {
                "🚫 Rejected (Feishu)"
            };
            let _ = notify_decision_resolved(decision_id, resolved_card(DecisionKind::PlanApproval, summary));
        }
        other => return Err(format!("unknown card action: {other}")),
    }
    Ok(b"{}".to_vec())
}

/// Locate the submitted form values inside a `card.action.trigger` body.
///
/// Feishu's exact field name for a `form_submit` payload has varied across
/// Card 2.0 revisions (`form_value` / `form_fields` / `form`), so probe the
/// documented locations in order and return the first object found.  This
/// is the one spot whose shape we could not confirm against a live Feishu
/// callback (no test credentials), so it parses defensively rather than
/// assuming a single path.
fn extract_form_value(body: &serde_json::Value) -> Option<&serde_json::Value> {
    for path in [
        "/event/action/form_value",
        "/event/action/form_fields",
        "/event/action/form",
        "/event/action/input_value",
    ] {
        if let Some(v) = body.pointer(path) {
            if v.is_object() {
                return Some(v);
            }
        }
    }
    None
}

/// Reassemble the elicitation answer map from the Submit button's `fields`
/// map (form field name → question text) and the form's submitted values
/// (`form_value`: field name → string for `select_static`, array for
/// `multi_select_static`).  Multi-select picks are joined with `", "` to
/// match the desktop encoding in `store.ts` `submitElicitation`.  Questions
/// the user left unanswered are omitted from the map.
///
/// Each question also has an optional `q{i}_other` free-text field.  When
/// non-empty it **overrides** the selector picks for that question — same
/// precedence as the desktop, where a `customAnswers` entry replaces the
/// selection rather than appending to it.
fn assemble_elicitation_answers(
    fields: &serde_json::Value,
    form_value: Option<&serde_json::Value>,
) -> HashMap<String, String> {
    let mut answers = HashMap::new();
    let Some(fields) = fields.as_object() else {
        return answers;
    };
    for (field_name, question) in fields {
        let Some(question) = question.as_str() else {
            continue;
        };

        // Free-text "Other" wins over the selector when filled.
        let custom = form_value
            .and_then(|fv| fv.get(format!("{field_name}_other")))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty());

        let joined = if let Some(custom) = custom {
            custom.to_string()
        } else {
            match form_value.and_then(|fv| fv.get(field_name)) {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Array(arr)) => arr
                    .iter()
                    .filter_map(|v| v.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                _ => continue, // unanswered → omit
            }
        };
        if !joined.is_empty() {
            answers.insert(question.to_string(), joined);
        }
    }
    answers
}

fn sha256_hex(s: &str) -> String {
    let digest = Sha256::digest(s.as_bytes());
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

// ── Card 2.0 transport ───────────────────────────────────────────────────────

/// Refresh-on-expiry tenant_access_token cache.  Concurrent callers may
/// occasionally double-fetch on cache miss; that is harmless — Feishu
/// just returns a fresh token both times and we keep the latter.
fn tenant_access_token() -> Result<String, String> {
    if let Some(cached) = bridge().tenant_token.lock().unwrap().clone() {
        if Instant::now() < cached.expires_at {
            return Ok(cached.token);
        }
    }
    let creds = AppCredentials::load()?;
    let client = http_client()?;
    let resp: serde_json::Value = client
        .post("https://open.feishu.cn/open-apis/auth/v3/tenant_access_token/internal")
        .json(&serde_json::json!({
            "app_id": creds.app_id,
            "app_secret": creds.app_secret,
        }))
        .send()
        .map_err(|e| format!("tenant_access_token: {e}"))?
        .json()
        .map_err(|e| format!("tenant_access_token parse: {e}"))?;
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = resp.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("tenant_access_token error code={code} msg={msg}"));
    }
    let token = resp
        .get("tenant_access_token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("tenant_access_token missing in response: {resp}"))?
        .to_string();
    let expire_secs = resp
        .get("expire")
        .and_then(|v| v.as_i64())
        .unwrap_or(7200);
    // Refresh 60s early to avoid edge-of-window failures.
    let safe = expire_secs.saturating_sub(60).max(60) as u64;
    *bridge().tenant_token.lock().unwrap() = Some(CachedTenantToken {
        token: token.clone(),
        expires_at: Instant::now() + Duration::from_secs(safe),
    });
    Ok(token)
}

/// Send an interactive (Card 2.0) message to `open_id` and return the
/// resulting `message_id`.
pub fn send_card(open_id: &str, card_json: &serde_json::Value) -> Result<String, String> {
    let token = tenant_access_token()?;
    let client = http_client()?;
    let resp: serde_json::Value = client
        .post("https://open.feishu.cn/open-apis/im/v1/messages")
        .query(&[("receive_id_type", "open_id")])
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "receive_id": open_id,
            "msg_type": "interactive",
            "content": card_json.to_string(),
        }))
        .send()
        .map_err(|e| format!("send_card: {e}"))?
        .json()
        .map_err(|e| format!("send_card parse: {e}"))?;
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = resp.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("send_card error code={code} msg={msg}"));
    }
    resp.get("data")
        .and_then(|d| d.get("message_id"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| format!("send_card missing data.message_id: {resp}"))
}

/// Replace the contents of an already-sent card.
pub fn update_card(message_id: &str, card_json: &serde_json::Value) -> Result<(), String> {
    let token = tenant_access_token()?;
    let client = http_client()?;
    let url = format!("https://open.feishu.cn/open-apis/im/v1/messages/{message_id}");
    let resp: serde_json::Value = client
        .patch(&url)
        .bearer_auth(&token)
        .json(&serde_json::json!({
            "content": card_json.to_string(),
        }))
        .send()
        .map_err(|e| format!("update_card: {e}"))?
        .json()
        .map_err(|e| format!("update_card parse: {e}"))?;
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = resp.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("update_card error code={code} msg={msg}"));
    }
    Ok(())
}

/// Trigger Feishu's "应用内紧急通知" on an already-sent message so the
/// user sees a forced banner / push.  Used for Guard cards only.
pub fn urgent_app(message_id: &str, open_ids: &[String]) -> Result<(), String> {
    if open_ids.is_empty() {
        return Ok(());
    }
    let token = tenant_access_token()?;
    let client = http_client()?;
    let url = format!(
        "https://open.feishu.cn/open-apis/im/v1/messages/{message_id}/urgent_app"
    );
    let resp: serde_json::Value = client
        .patch(&url)
        .query(&[("user_id_type", "open_id")])
        .bearer_auth(&token)
        .json(&serde_json::json!({ "user_id_list": open_ids }))
        .send()
        .map_err(|e| format!("urgent_app: {e}"))?
        .json()
        .map_err(|e| format!("urgent_app parse: {e}"))?;
    let code = resp.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    if code != 0 {
        let msg = resp.get("msg").and_then(|v| v.as_str()).unwrap_or("");
        return Err(format!("urgent_app error code={code} msg={msg}"));
    }
    Ok(())
}

fn http_client() -> Result<reqwest::blocking::Client, String> {
    reqwest::blocking::Client::builder()
        .timeout(FEISHU_HTTP_TIMEOUT)
        .build()
        .map_err(|e| format!("http client init: {e}"))
}

// ── Card 2.0 builders ────────────────────────────────────────────────────────

#[derive(Serialize, Debug, Clone)]
pub struct GuardCard {
    pub workspace: String,
    pub command: String,
    pub risk_label: Option<String>,
    pub llm_analysis: Option<String>,
    pub decision_id: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct ElicitationOptionCard {
    pub label: String,
    pub description: Option<String>,
    pub value: String,
}

/// One question inside a multi-question elicitation form.
#[derive(Serialize, Debug, Clone)]
pub struct ElicitationQuestionCard {
    /// Question text — also the key under which the answer is written into
    /// `ElicitationResponse.answers`.
    pub question: String,
    pub options: Vec<ElicitationOptionCard>,
    pub multi_select: bool,
}

#[derive(Serialize, Debug, Clone)]
pub struct ElicitationCard {
    pub workspace: String,
    /// All questions rendered onto a single card as one form; one Submit
    /// returns every answer at once (no server-side cross-webhook state).
    pub questions: Vec<ElicitationQuestionCard>,
    pub decision_id: String,
}

#[derive(Serialize, Debug, Clone)]
pub struct PlanCard {
    pub workspace: String,
    pub plan_markdown: String,
    pub decision_id: String,
}

impl GuardCard {
    pub fn to_card_json(&self) -> serde_json::Value {
        let mut body = String::new();
        body.push_str(&format!("**工作区:** {}\n\n", escape_md(&self.workspace)));
        if let Some(risk) = &self.risk_label {
            body.push_str(&format!("**风险:** {}\n\n", escape_md(risk)));
        }
        body.push_str(&format!("**命令:**\n```\n{}\n```", self.command));
        if let Some(analysis) = &self.llm_analysis {
            body.push_str(&format!("\n\n**分析:**\n{}", analysis));
        }

        let allow_value = serde_json::json!({
            "action": "guard",
            "decision_id": self.decision_id,
            "allow": true,
        });
        let block_value = serde_json::json!({
            "action": "guard",
            "decision_id": self.decision_id,
            "allow": false,
        });

        serde_json::json!({
            "schema": "2.0",
            "config": { "wide_screen_mode": true },
            "header": {
                "template": "red",
                "title": { "tag": "plain_text", "content": format!("授权操作 · {}", short(&self.command, 40)) }
            },
            "body": {
                "elements": [
                    { "tag": "markdown", "content": body },
                    {
                        "tag": "column_set",
                        "horizontal_spacing": "8px",
                        "columns": [
                            {
                                "tag": "column",
                                "width": "weighted",
                                "weight": 1,
                                "elements": [callback_button("✅ Allow", "primary_filled", allow_value)]
                            },
                            {
                                "tag": "column",
                                "width": "weighted",
                                "weight": 1,
                                "elements": [callback_button("🚫 Block", "danger", block_value)]
                            }
                        ]
                    }
                ]
            }
        })
    }
}

impl ElicitationCard {
    /// Render every question onto one card as a single Feishu Card 2.0
    /// `form`.  Single-select questions use `select_static`, multi-select
    /// use `multi_select_static`; one `form_submit` button returns all
    /// answers at once.  The Submit button's callback value carries a
    /// `fields` map (`q0` → question text, …) so the webhook handler can
    /// reassemble `answers` without keeping any server-side state.
    ///
    /// Each option's `value` is set to the option label itself, so the
    /// submitted form value IS the human-readable label — no value→label
    /// lookup is needed at resolve time.
    pub fn to_card_json(&self) -> serde_json::Value {
        // Fast path: a lone single-select question renders as a flat list of
        // callback buttons (one per option) instead of a collapsed
        // `select_static` dropdown — every option *and* its description is
        // visible at once, and a tap resolves the decision immediately via the
        // handler's single-answer (`question` + `answer_label`) path.  Only
        // single-select is eligible: multi-select and multi-question cards
        // still need the `form` + one Submit so every answer comes back
        // together, and Feishu has no verified inline form-submittable
        // single-select primitive other than the dropdown.
        if self.questions.len() == 1 && !self.questions[0].multi_select {
            return self.to_buttons_card_json();
        }

        let mut elements: Vec<serde_json::Value> = vec![serde_json::json!({
            "tag": "markdown",
            "content": format!("**工作区:** {}", escape_md(&self.workspace)),
        })];

        let multi = self.questions.len() > 1;
        let mut form_elements: Vec<serde_json::Value> = Vec::new();
        let mut fields = serde_json::Map::new();

        for (i, q) in self.questions.iter().enumerate() {
            let field_name = format!("q{i}");
            fields.insert(
                field_name.clone(),
                serde_json::Value::String(q.question.clone()),
            );

            let prompt = if multi {
                format!("**{}. {}**", i + 1, escape_md(&q.question))
            } else {
                format!("**{}**", escape_md(&q.question))
            };
            form_elements.push(serde_json::json!({ "tag": "markdown", "content": prompt }));

            let options: Vec<serde_json::Value> = q
                .options
                .iter()
                .map(|opt| {
                    let text = match &opt.description {
                        Some(d) => format!("{} — {}", opt.label, d),
                        None => opt.label.clone(),
                    };
                    serde_json::json!({
                        "text": { "tag": "plain_text", "content": short(&text, 120) },
                        // value == label so the submitted value is the answer text.
                        "value": opt.label,
                    })
                })
                .collect();

            let (tag, placeholder) = if q.multi_select {
                ("multi_select_static", "可多选")
            } else {
                ("select_static", "选择一项")
            };
            form_elements.push(serde_json::json!({
                "tag": tag,
                "name": field_name,
                "placeholder": { "tag": "plain_text", "content": placeholder },
                "options": options,
            }));

            // Optional free-text "Other" input, mirroring the implicit
            // Other choice AskUserQuestion appends to every question (the
            // desktop exposes it via `customAnswers`).  If filled, it
            // overrides the selector picks at resolve time.
            form_elements.push(serde_json::json!({
                "tag": "input",
                "name": format!("{field_name}_other"),
                "required": false,
                "placeholder": { "tag": "plain_text", "content": "其它（自己填写，将覆盖上面的选择）" },
            }));
        }

        let submit_value = serde_json::json!({
            "action": "elicitation",
            "decision_id": self.decision_id,
            "fields": serde_json::Value::Object(fields),
        });
        form_elements.push(serde_json::json!({
            "tag": "button",
            // A submit button inside a `form` container is itself a form-
            // interactive component, so it needs a unique non-empty `name`
            // like every selector/input — without it Feishu rejects the
            // submit client-side with error 200530 and never delivers the
            // card.action.trigger callback.
            "name": "submit",
            "text": { "tag": "plain_text", "content": "✅ 提交" },
            "type": "primary",
            "width": "fill",
            "form_action_type": "submit",
            "behaviors": [ { "type": "callback", "value": submit_value } ],
        }));

        elements.push(serde_json::json!({
            "tag": "form",
            "name": "elicitation_form",
            "elements": form_elements,
        }));

        wrap_card("Decision needed", "blue", elements)
    }

    /// Render a lone single-select question as one full-width callback button
    /// per option.  Each option's description (when present) is shown as a
    /// markdown line directly above its button, so nothing is truncated into
    /// the button label.  The button's callback value carries `question` +
    /// `answer_label`, which `handle_card_action` resolves through its
    /// single-answer path — no `form`, no Submit, one tap settles it.
    fn to_buttons_card_json(&self) -> serde_json::Value {
        let q = &self.questions[0];
        let mut elements: Vec<serde_json::Value> = vec![
            serde_json::json!({
                "tag": "markdown",
                "content": format!("**工作区:** {}", escape_md(&self.workspace)),
            }),
            serde_json::json!({
                "tag": "markdown",
                "content": format!("**{}**", escape_md(&q.question)),
            }),
        ];

        for opt in &q.options {
            if let Some(desc) = &opt.description {
                elements.push(serde_json::json!({
                    "tag": "markdown",
                    "content": format!("**{}** — {}", escape_md(&opt.label), escape_md(desc)),
                }));
            }
            let value = serde_json::json!({
                "action": "elicitation",
                "decision_id": self.decision_id,
                "question": q.question,
                "answer_label": opt.label,
            });
            elements.push(callback_button(&opt.label, "default", value));
        }

        wrap_card("Decision needed", "blue", elements)
    }
}

impl PlanCard {
    pub fn to_card_json(&self) -> serde_json::Value {
        let body = format!(
            "**工作区:** {}\n\n---\n\n{}",
            escape_md(&self.workspace),
            self.plan_markdown,
        );
        let approve_value = serde_json::json!({
            "action": "plan",
            "decision_id": self.decision_id,
            "decision": "approve",
        });
        let reject_value = serde_json::json!({
            "action": "plan",
            "decision_id": self.decision_id,
            "decision": "reject",
        });

        serde_json::json!({
            "schema": "2.0",
            "config": { "wide_screen_mode": true },
            "header": {
                "template": "violet",
                "title": { "tag": "plain_text", "content": "Plan review" }
            },
            "body": {
                "elements": [
                    { "tag": "markdown", "content": body },
                    {
                        "tag": "column_set",
                        "horizontal_spacing": "8px",
                        "columns": [
                            {
                                "tag": "column",
                                "width": "weighted",
                                "weight": 1,
                                "elements": [callback_button("✅ Approve", "primary_filled", approve_value)]
                            },
                            {
                                "tag": "column",
                                "width": "weighted",
                                "weight": 1,
                                "elements": [callback_button("🚫 Reject", "danger", reject_value)]
                            }
                        ]
                    }
                ]
            }
        })
    }
}

fn escape_md(s: &str) -> String {
    s.replace('*', "\\*").replace('_', "\\_").replace('`', "\\`")
}

fn short(s: &str, max: usize) -> String {
    let trimmed: String = s.chars().take(max).collect();
    if trimmed.chars().count() < s.chars().count() {
        format!("{trimmed}…")
    } else {
        trimmed
    }
}

fn callback_button(
    text: &str,
    button_type: &str,
    value: serde_json::Value,
) -> serde_json::Value {
    serde_json::json!({
        "tag": "button",
        "text": { "tag": "plain_text", "content": text },
        "type": button_type,
        "width": "fill",
        "behaviors": [
            { "type": "callback", "value": value }
        ]
    })
}

fn wrap_card(title: &str, template: &str, elements: Vec<serde_json::Value>) -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "config": { "wide_screen_mode": true },
        "header": {
            "template": template,
            "title": { "tag": "plain_text", "content": title }
        },
        "body": { "elements": elements }
    })
}

// ── Decision lifecycle bridge ────────────────────────────────────────────────

/// Sends `card_json` to the bound user (no-op if not connected), records
/// the resulting `message_id` against `decision_id`, and triggers
/// `urgent_app` for `Guard`-kind decisions.  Failures are returned but
/// callers in the watcher loop should log-and-continue rather than abort.
///
/// Race handling: if `notify_decision_resolved` runs while we're still
/// in `send_card`, it leaves a `ResolvedBeforeSent` marker in the state
/// map; on the back side of the send we apply the read-only update
/// inline so the user's phone still flips.
pub fn notify_decision_created(
    decision_id: &str,
    kind: DecisionKind,
    card_json: &serde_json::Value,
) -> Result<(), String> {
    let Some(open_id) = bound_open_id() else {
        return Ok(());
    };
    // Reserve the slot before the slow HTTP call so a concurrent
    // `notify_decision_resolved` knows a card is on the wire.
    bridge()
        .decision_states
        .lock()
        .unwrap()
        .insert(decision_id.to_string(), DecisionState::Sending);

    let send_result = send_card(&open_id, card_json);

    match send_result {
        Ok(message_id) => {
            // Atomically transition Sending → Sent or, if a resolve
            // raced ahead, swallow the marker and flip the card.
            let pending_resolve = {
                let mut map = bridge().decision_states.lock().unwrap();
                let prior = map.remove(decision_id);
                match prior {
                    Some(DecisionState::ResolvedBeforeSent { card }) => Some(card),
                    _ => {
                        map.insert(
                            decision_id.to_string(),
                            DecisionState::Sent {
                                message_id: message_id.clone(),
                            },
                        );
                        None
                    }
                }
            };

            if let Some(card) = pending_resolve {
                return update_card(&message_id, &card);
            }
            if matches!(kind, DecisionKind::Guard) {
                // Guard is the only kind that warrants the forced banner.
                // Failure here is non-fatal — the card itself is already
                // delivered.
                let _ = urgent_app(&message_id, &[open_id]);
            }
            Ok(())
        }
        Err(e) => {
            // Drop the reservation so a later resolve doesn't dangle.
            bridge()
                .decision_states
                .lock()
                .unwrap()
                .remove(decision_id);
            Err(e)
        }
    }
}

/// Flips the previously-sent card into a read-only "已处理" state, applying
/// the caller-rendered `card`.  If the send is still in flight, stores the
/// card so the send-thread applies it once it has a `message_id`.  No-op if
/// no card was ever sent (e.g. not connected).
pub fn notify_decision_resolved(
    decision_id: &str,
    card: serde_json::Value,
) -> Result<(), String> {
    let message_id = {
        let mut map = bridge().decision_states.lock().unwrap();
        match map.get(decision_id).cloned() {
            Some(DecisionState::Sent { message_id }) => {
                map.remove(decision_id);
                Some(message_id)
            }
            Some(DecisionState::Sending) => {
                // Send still in flight — stash the rendered card; the
                // send-thread applies it once it has a message_id.
                map.insert(
                    decision_id.to_string(),
                    DecisionState::ResolvedBeforeSent { card },
                );
                return Ok(());
            }
            // Already-marked or never-existed both no-op.
            _ => None,
        }
    };
    let Some(message_id) = message_id else {
        return Ok(());
    };
    update_card(&message_id, &card)
}

pub fn resolved_card(kind: DecisionKind, summary: &str) -> serde_json::Value {
    serde_json::json!({
        "schema": "2.0",
        "config": { "wide_screen_mode": true },
        "header": {
            "template": "grey",
            "title": { "tag": "plain_text", "content": format!("{} · 已处理", kind.label()) }
        },
        "body": {
            "elements": [
                { "tag": "markdown", "content": format!("**已在 Fleet 桌面端处理。**\n\n{summary}") }
            ]
        }
    })
}

/// Strip the AskUserQuestion speech-summary divider (`\n---\n`) from a
/// question, returning the real question body (the part after the last
/// divider).  Cards show the body, not the spoken pre-divider summary.
fn question_body(question: &str) -> &str {
    match question.rsplit_once("\n---\n") {
        Some((_, body)) => body.trim(),
        None => question.trim(),
    }
}

/// Build the read-only "已处理" card for a resolved elicitation, preserving
/// the original questions and *all* their options (greyed out) with the
/// submitted pick(s) highlighted — instead of collapsing to a single answer
/// line.  `answers` maps question text → submitted label(s) (multi-select
/// joined with `, `).  Reads the original questions/options from the pending
/// request file; if it is already gone, falls back to the compact summary
/// card so we still flip the card to a resolved state.
pub fn resolved_elicitation_card(
    decision_id: &str,
    answers: &HashMap<String, String>,
    declined: bool,
) -> serde_json::Value {
    let Some(request) = crate::elicitation::read_request(decision_id) else {
        // Request already cleaned up — degrade to the compact summary card.
        let summary = if declined {
            "↩︎ 未选择 (Feishu)".to_string()
        } else if answers.is_empty() {
            "已回复 (Feishu)".to_string()
        } else {
            let mut picks: Vec<&str> = answers.values().map(|s| s.as_str()).collect();
            picks.sort_unstable();
            format!("✅ {} (Feishu)", picks.join(" · "))
        };
        return resolved_card(DecisionKind::Elicitation, &summary);
    };
    render_resolved_elicitation(&request, answers, declined)
}

/// Pure renderer for the resolved elicitation card — split from
/// [`resolved_elicitation_card`] so it can be tested without a request file
/// on disk.
fn render_resolved_elicitation(
    request: &crate::elicitation::ElicitationRequest,
    answers: &HashMap<String, String>,
    declined: bool,
) -> serde_json::Value {
    let mut body = format!(
        "**已在 Fleet 桌面端处理。**\n\n**工作区:** {}",
        escape_md(&request.workspace_name)
    );
    let multi_q = request.questions.len() > 1;

    for (i, q) in request.questions.iter().enumerate() {
        let prompt = if multi_q {
            format!("**{}. {}**", i + 1, escape_md(question_body(&q.question)))
        } else {
            format!("**{}**", escape_md(question_body(&q.question)))
        };
        body.push_str("\n\n");
        body.push_str(&prompt);

        // The submitted pick(s) for this question; multi-select arrives as a
        // single string joined with ", " (see `assemble_elicitation_answers`).
        let picked: Vec<String> = answers
            .get(&q.question)
            .map(|a| a.split(", ").map(|s| s.trim().to_string()).collect())
            .unwrap_or_default();

        for opt in &q.options {
            let chosen = picked.iter().any(|p| p == &opt.label);
            let line = if opt.description.is_empty() {
                opt.label.clone()
            } else {
                format!("{} — {}", opt.label, opt.description)
            };
            if chosen {
                body.push_str(&format!("\n✅ **{}**", escape_md(&short(&line, 120))));
            } else {
                // Greyed/struck-through to read as "not chosen".
                body.push_str(&format!("\n· ~~{}~~", escape_md(&short(&line, 120))));
            }
        }

        // A free-text "Other" answer that matches no option label.
        for p in &picked {
            if !q.options.iter().any(|o| &o.label == p) && !p.is_empty() {
                body.push_str(&format!("\n✅ **其它:** {}", escape_md(&short(p, 120))));
            }
        }

        if picked.is_empty() {
            body.push_str("\n· （未选择）");
        }
    }

    if declined {
        body.push_str("\n\n↩︎ **未选择，已按取消处理。**");
    }

    serde_json::json!({
        "schema": "2.0",
        "config": { "wide_screen_mode": true },
        "header": {
            "template": "grey",
            "title": { "tag": "plain_text", "content": format!("{} · 已处理", DecisionKind::Elicitation.label()) }
        },
        "body": { "elements": [ { "tag": "markdown", "content": body } ] }
    })
}

// ── Persistence ──────────────────────────────────────────────────────────────

fn persistence_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("feishu.json"))
}

fn load_persistence() -> Option<BoundIdentity> {
    let path = persistence_path()?;
    let data = std::fs::read_to_string(&path).ok()?;
    let v: serde_json::Value = serde_json::from_str(&data).ok()?;
    Some(BoundIdentity {
        open_id: v.get("open_id")?.as_str()?.to_string(),
        name: v.get("name").and_then(|n| n.as_str()).map(|s| s.to_string()),
    })
}

fn save_persistence(b: &BoundIdentity) -> Result<(), String> {
    let path = persistence_path().ok_or("cannot determine home dir")?;
    let parent = path.parent().ok_or("no parent dir")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let body = serde_json::json!({"open_id": b.open_id, "name": b.name}).to_string();
    std::fs::write(&path, body).map_err(|e| e.to_string())
}

fn delete_persistence() {
    if let Some(path) = persistence_path() {
        let _ = std::fs::remove_file(&path);
    }
}

fn creds_path() -> Option<PathBuf> {
    crate::session::real_home_dir().map(|h| h.join(".fleet").join("feishu-creds.json"))
}

fn load_creds_file() -> Option<StoredCreds> {
    let path = creds_path()?;
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str::<StoredCreds>(&data).ok()
}

fn save_creds_file(c: &StoredCreds) -> Result<(), String> {
    let path = creds_path().ok_or("cannot determine home dir")?;
    let parent = path.parent().ok_or("no parent dir")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let body = serde_json::to_string(c).map_err(|e| e.to_string())?;
    std::fs::write(&path, body).map_err(|e| e.to_string())?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

fn delete_creds_file() {
    if let Some(path) = creds_path() {
        let _ = std::fs::remove_file(&path);
    }
}

// ── Helpers ──────────────────────────────────────────────────────────────────

fn mint_state() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

fn mint_code_verifier() -> String {
    let a = uuid::Uuid::new_v4();
    let b = uuid::Uuid::new_v4();
    let mut bytes = [0u8; 32];
    bytes[..16].copy_from_slice(a.as_bytes());
    bytes[16..].copy_from_slice(b.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn pkce_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest)
}

fn url_encode(s: &str) -> String {
    percent_encoding::utf8_percent_encode(s, percent_encoding::NON_ALPHANUMERIC).to_string()
}

fn parse_query(query: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        if let Some((k, v)) = pair.split_once('=') {
            let key = percent_encoding::percent_decode_str(k)
                .decode_utf8_lossy()
                .into_owned();
            let val = percent_encoding::percent_decode_str(v)
                .decode_utf8_lossy()
                .into_owned();
            map.insert(key, val);
        }
    }
    map
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// PKCE S256 against RFC 7636 Appendix B test vector
    /// (`dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk`).
    #[test]
    fn pkce_s256_rfc7636_vector() {
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let challenge = pkce_s256(verifier);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

    /// Code verifier should be ≥43 characters per RFC 7636 §4.1.
    #[test]
    fn code_verifier_meets_rfc_length() {
        let v = mint_code_verifier();
        assert!(v.len() >= 43, "verifier too short: {} chars", v.len());
        assert!(v.len() <= 128);
    }

    #[test]
    fn parse_query_handles_percent_decoding() {
        let q = parse_query("state=abc&code=hello%20world");
        assert_eq!(q.get("state").map(String::as_str), Some("abc"));
        assert_eq!(q.get("code").map(String::as_str), Some("hello world"));
    }

    /// Card buttons must embed a `behaviors[0].value.action` payload so
    /// the webhook can dispatch.  Regression: the v1 card stub had an
    /// empty `body.elements` and dropped the payload silently.
    #[test]
    fn guard_card_carries_callback_payload() {
        let card = GuardCard {
            workspace: "demo".into(),
            command: "rm -rf /".into(),
            risk_label: Some("destructive".into()),
            llm_analysis: None,
            decision_id: "g-123".into(),
        }
        .to_card_json();
        let buttons: Vec<&serde_json::Value> = card
            .pointer("/body/elements")
            .and_then(|e| e.as_array())
            .unwrap()
            .iter()
            .filter(|el| el.get("tag").and_then(|t| t.as_str()) == Some("column_set"))
            .flat_map(|cs| cs.get("columns").and_then(|c| c.as_array()).unwrap().iter())
            .flat_map(|col| col.get("elements").and_then(|e| e.as_array()).unwrap().iter())
            .collect();
        assert_eq!(buttons.len(), 2, "guard card should have 2 buttons");
        for btn in &buttons {
            let value = btn
                .pointer("/behaviors/0/value")
                .expect("button missing callback value");
            assert_eq!(
                value.get("action").and_then(|v| v.as_str()),
                Some("guard")
            );
            assert_eq!(
                value.get("decision_id").and_then(|v| v.as_str()),
                Some("g-123")
            );
            assert!(value.get("allow").and_then(|v| v.as_bool()).is_some());
        }
    }

    #[test]
    fn elicitation_card_renders_form_with_one_selector_per_question() {
        let card = ElicitationCard {
            workspace: "demo".into(),
            questions: vec![
                ElicitationQuestionCard {
                    question: "Which framework?".into(),
                    options: vec![
                        ElicitationOptionCard {
                            label: "A".into(),
                            description: None,
                            value: "A".into(),
                        },
                        ElicitationOptionCard {
                            label: "B".into(),
                            description: Some("the second".into()),
                            value: "B".into(),
                        },
                    ],
                    multi_select: false,
                },
                ElicitationQuestionCard {
                    question: "Which features?".into(),
                    options: vec![ElicitationOptionCard {
                        label: "Auth".into(),
                        description: None,
                        value: "Auth".into(),
                    }],
                    multi_select: true,
                },
            ],
            decision_id: "e-9".into(),
        }
        .to_card_json();

        // The card body holds a single form element.
        let form = card
            .pointer("/body/elements")
            .and_then(|e| e.as_array())
            .unwrap()
            .iter()
            .find(|el| el.get("tag").and_then(|t| t.as_str()) == Some("form"))
            .expect("form element present");
        let form_elements = form
            .pointer("/elements")
            .and_then(|e| e.as_array())
            .unwrap();

        // One select_static (single) + one multi_select_static (multi).
        let selectors: Vec<&str> = form_elements
            .iter()
            .filter_map(|el| el.get("tag").and_then(|t| t.as_str()))
            .filter(|t| *t == "select_static" || *t == "multi_select_static")
            .collect();
        assert_eq!(selectors, vec!["select_static", "multi_select_static"]);

        // Each question gets an optional free-text "Other" input named
        // `q{i}_other`.
        let other_inputs: Vec<&str> = form_elements
            .iter()
            .filter(|el| el.get("tag").and_then(|t| t.as_str()) == Some("input"))
            .filter_map(|el| el.get("name").and_then(|n| n.as_str()))
            .collect();
        assert_eq!(other_inputs, vec!["q0_other", "q1_other"]);

        // Selector field names are q0/q1 and option value == label.
        let q0 = form_elements
            .iter()
            .find(|el| el.get("name").and_then(|n| n.as_str()) == Some("q0"))
            .unwrap();
        assert_eq!(q0.get("tag").and_then(|t| t.as_str()), Some("select_static"));
        let opt_values: Vec<&str> = q0
            .pointer("/options")
            .and_then(|o| o.as_array())
            .unwrap()
            .iter()
            .filter_map(|o| o.get("value").and_then(|v| v.as_str()))
            .collect();
        assert_eq!(opt_values, vec!["A", "B"]);

        // The single Submit button carries the fields map + decision_id.
        let submit = form_elements
            .iter()
            .find(|el| el.get("form_action_type").and_then(|t| t.as_str()) == Some("submit"))
            .expect("submit button present");
        let value = submit.pointer("/behaviors/0/value").unwrap();
        assert_eq!(value.get("action").and_then(|x| x.as_str()), Some("elicitation"));
        assert_eq!(value.get("decision_id").and_then(|x| x.as_str()), Some("e-9"));
        assert_eq!(
            value.pointer("/fields/q0").and_then(|x| x.as_str()),
            Some("Which framework?")
        );
        assert_eq!(
            value.pointer("/fields/q1").and_then(|x| x.as_str()),
            Some("Which features?")
        );
    }

    #[test]
    fn single_select_single_question_renders_option_buttons_not_dropdown() {
        // A lone single-select question must render every option as its own
        // callback button (all options + descriptions visible, no collapsed
        // dropdown, no form) so the user can answer with a single tap.
        let card = ElicitationCard {
            workspace: "demo".into(),
            questions: vec![ElicitationQuestionCard {
                question: "Which framework?".into(),
                options: vec![
                    ElicitationOptionCard {
                        label: "Alpha".into(),
                        description: Some("light & fast".into()),
                        value: "Alpha".into(),
                    },
                    ElicitationOptionCard {
                        label: "Beta".into(),
                        description: None,
                        value: "Beta".into(),
                    },
                ],
                multi_select: false,
            }],
            decision_id: "e-btn".into(),
        }
        .to_card_json();

        let elements = card
            .pointer("/body/elements")
            .and_then(|e| e.as_array())
            .unwrap();

        // No form, no dropdown anywhere on the card.
        assert!(
            elements.iter().all(|el| {
                let t = el.get("tag").and_then(|t| t.as_str()).unwrap_or("");
                t != "form" && t != "select_static" && t != "multi_select_static"
            }),
            "single-select single-question card must not use a form or dropdown"
        );

        // One callback button per option, in order, label == option label.
        let buttons: Vec<&serde_json::Value> = elements
            .iter()
            .filter(|el| el.get("tag").and_then(|t| t.as_str()) == Some("button"))
            .collect();
        assert_eq!(buttons.len(), 2, "one button per option");
        let labels: Vec<&str> = buttons
            .iter()
            .filter_map(|b| b.pointer("/text/content").and_then(|c| c.as_str()))
            .collect();
        assert_eq!(labels, vec!["Alpha", "Beta"]);

        // Each button carries the single-answer payload the handler's compat
        // path consumes: action + decision_id + question + answer_label.
        for (btn, label) in buttons.iter().zip(["Alpha", "Beta"]) {
            let value = btn
                .pointer("/behaviors/0/value")
                .expect("button missing callback value");
            assert_eq!(value.get("action").and_then(|v| v.as_str()), Some("elicitation"));
            assert_eq!(value.get("decision_id").and_then(|v| v.as_str()), Some("e-btn"));
            assert_eq!(
                value.get("question").and_then(|v| v.as_str()),
                Some("Which framework?")
            );
            assert_eq!(value.get("answer_label").and_then(|v| v.as_str()), Some(label));
        }

        // The description of the first option appears as visible markdown text.
        let has_desc = elements.iter().any(|el| {
            el.get("tag").and_then(|t| t.as_str()) == Some("markdown")
                && el
                    .get("content")
                    .and_then(|c| c.as_str())
                    .map(|c| c.contains("light & fast"))
                    .unwrap_or(false)
        });
        assert!(has_desc, "option description must be rendered as markdown");
    }

    #[test]
    fn elicitation_form_interactive_components_all_have_unique_nonempty_names() {
        // Feishu rejects a form submit with error 200530 when any interactive
        // component inside a `form` container has an empty/missing `name`
        // (names must also be unique). The submit button carries
        // `form_action_type: "submit"`, which makes it a form-interactive
        // component — so it MUST have a name too, not just the selectors and
        // inputs. Regression guard for the 200530 card-callback failure.
        // Multi-select keeps the card on the `form` path (single-select single-
        // question now renders as buttons with no form), so this still guards
        // the form's name-uniqueness invariant.
        let card = ElicitationCard {
            workspace: "demo".into(),
            questions: vec![ElicitationQuestionCard {
                question: "Which framework?".into(),
                options: vec![ElicitationOptionCard {
                    label: "A".into(),
                    description: None,
                    value: "A".into(),
                }],
                multi_select: true,
            }],
            decision_id: "e-1".into(),
        }
        .to_card_json();

        let form = card
            .pointer("/body/elements")
            .and_then(|e| e.as_array())
            .unwrap()
            .iter()
            .find(|el| el.get("tag").and_then(|t| t.as_str()) == Some("form"))
            .expect("form element present");
        let form_elements = form
            .pointer("/elements")
            .and_then(|e| e.as_array())
            .unwrap();

        // An element is "interactive" if it is a known input/select tag or a
        // button acting as the form's submit trigger. Pure display elements
        // (markdown) are exempt.
        let mut seen_names = std::collections::HashSet::new();
        for el in form_elements {
            let tag = el.get("tag").and_then(|t| t.as_str()).unwrap_or("");
            let is_submit_button = tag == "button"
                && el.get("form_action_type").and_then(|t| t.as_str()) == Some("submit");
            let is_interactive = matches!(
                tag,
                "select_static" | "multi_select_static" | "input"
            ) || is_submit_button;
            if !is_interactive {
                continue;
            }
            let name = el.get("name").and_then(|n| n.as_str()).unwrap_or("");
            assert!(
                !name.is_empty(),
                "interactive form component (tag={tag}) must have a non-empty name; \
                 an empty name triggers Feishu error 200530"
            );
            assert!(
                seen_names.insert(name.to_string()),
                "form component name `{name}` is duplicated; names must be unique"
            );
        }
    }

    #[test]
    fn resolved_elicitation_card_keeps_questions_and_marks_picks() {
        use crate::elicitation::{ElicitationOption, ElicitationQuestion, ElicitationRequest};
        let request = ElicitationRequest {
            parked: false,
            id: "e-1".into(),
            session_id: "s".into(),
            workspace_name: "demo".into(),
            ai_title: None,
            timestamp: "t".into(),
            questions: vec![ElicitationQuestion {
                // Carries the speech-summary divider; only the body should show.
                question: "语音摘要句\n---\n选哪个框架？".into(),
                header: "框架".into(),
                multi_select: false,
                options: vec![
                    ElicitationOption {
                        label: "React".into(),
                        description: "前端".into(),
                        preview: None,
                    },
                    ElicitationOption {
                        label: "Vue".into(),
                        description: String::new(),
                        preview: None,
                    },
                ],
            }],
        };
        let mut answers = HashMap::new();
        answers.insert("语音摘要句\n---\n选哪个框架？".to_string(), "React".to_string());

        let card = render_resolved_elicitation(&request, &answers, false);
        let content = card
            .pointer("/body/elements/0/content")
            .and_then(|c| c.as_str())
            .unwrap();

        // The real question body is shown; the spoken pre-divider summary is not.
        assert!(content.contains("选哪个框架？"), "question body must remain: {content}");
        assert!(!content.contains("语音摘要句"), "pre-divider summary must be stripped");
        // The original options are all preserved.
        assert!(content.contains("React"), "picked option present");
        assert!(content.contains("Vue"), "unpicked option still present (greyed)");
        // The pick is highlighted and the other is struck through.
        assert!(content.contains("✅ **React"), "picked option highlighted: {content}");
        assert!(content.contains("~~Vue~~"), "unpicked option struck through: {content}");
        // Header marks it resolved, and the card is non-interactive (no form).
        assert_eq!(
            card.pointer("/header/title/content").and_then(|c| c.as_str()),
            Some("Decision needed · 已处理")
        );
        assert!(card.pointer("/body/elements/0/tag").and_then(|t| t.as_str()) == Some("markdown"));
        let body_str = serde_json::to_string(&card).unwrap();
        assert!(!body_str.contains("form_action_type"), "resolved card must not be interactive");
    }

    #[test]
    fn assemble_answers_handles_single_and_multi_select() {
        let fields = serde_json::json!({
            "q0": "Which framework?",
            "q1": "Which features?",
            "q2": "Skipped question?",
        });
        let form_value = serde_json::json!({
            "q0": "React",
            "q1": ["Auth", "Billing"],
            // q2 intentionally absent → unanswered, must be omitted.
        });
        let answers = assemble_elicitation_answers(&fields, Some(&form_value));
        assert_eq!(answers.len(), 2);
        assert_eq!(answers.get("Which framework?").map(String::as_str), Some("React"));
        // Multi-select joined with ", " to match the desktop encoding.
        assert_eq!(
            answers.get("Which features?").map(String::as_str),
            Some("Auth, Billing")
        );
        assert!(!answers.contains_key("Skipped question?"));
    }

    #[test]
    fn assemble_answers_empty_when_nothing_submitted() {
        let fields = serde_json::json!({ "q0": "Which?" });
        assert!(assemble_elicitation_answers(&fields, None).is_empty());
    }

    #[test]
    fn assemble_answers_other_text_overrides_selector() {
        let fields = serde_json::json!({ "q0": "Which framework?", "q1": "Pick one?" });
        let form_value = serde_json::json!({
            "q0": "React",
            "q0_other": "  Svelte  ",   // non-empty → overrides q0
            "q1": "A",
            "q1_other": "   ",           // whitespace only → ignored
        });
        let answers = assemble_elicitation_answers(&fields, Some(&form_value));
        // Trimmed custom text replaces the selector pick for q0.
        assert_eq!(answers.get("Which framework?").map(String::as_str), Some("Svelte"));
        // Blank Other leaves the selector value intact for q1.
        assert_eq!(answers.get("Pick one?").map(String::as_str), Some("A"));
    }

    #[test]
    fn extract_form_value_probes_documented_paths() {
        // form_fields fallback path.
        let body = serde_json::json!({
            "event": { "action": { "form_fields": { "q0": "A" } } }
        });
        assert_eq!(
            extract_form_value(&body).and_then(|v| v.get("q0")).and_then(|v| v.as_str()),
            Some("A")
        );
        // No recognised path → None.
        let empty = serde_json::json!({ "event": { "action": {} } });
        assert!(extract_form_value(&empty).is_none());
    }

    /// URL verification handshake — Feishu console probes the endpoint
    /// before accepting it.  Without `FEISHU_VERIFICATION_TOKEN` we still
    /// echo the challenge (the operator can add the token later).
    /// Tests that exercise `handle_webhook` go through `AppCredentials::load()`,
    /// which checks `<FLEET_HOME>/.fleet/feishu-creds.json` before falling
    /// back to env vars.  We can't share one FLEET_HOME because tests run
    /// in parallel and `stored_creds_round_trip` writes a file there — a
    /// concurrent `url_verification_*` test would then read those creds
    /// instead of its own env.  Mutex-serializing the env-var-touching tests
    /// is the lightest fix.
    // Shared with all other modules via `crate::session::fleet_home_lock()`
    // so we don't race against decision_history / supervisor / master tests
    // that also mutate $FLEET_HOME.
    fn isolate_creds_file(slot: &str) {
        let dir = std::env::temp_dir().join(format!("claw-fleet-feishu-{slot}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("FLEET_HOME", &dir);
    }

    #[test]
    fn url_verification_echoes_challenge() {
        let _g = crate::session::fleet_home_lock();
        isolate_creds_file("echo");
        std::env::set_var("FEISHU_APP_ID", "tst");
        std::env::set_var("FEISHU_APP_SECRET", "tst");
        std::env::remove_var("FEISHU_VERIFICATION_TOKEN");
        std::env::remove_var("FEISHU_ENCRYPT_KEY");

        let body = br#"{"type":"url_verification","challenge":"hello-world","token":"x"}"#;
        let resp = handle_webhook(body, None, None, None).expect("ok response");
        let json: serde_json::Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(
            json.get("challenge").and_then(|v| v.as_str()),
            Some("hello-world")
        );
    }

    #[test]
    fn url_verification_rejects_bad_token() {
        let _g = crate::session::fleet_home_lock();
        isolate_creds_file("bad-token");
        std::env::set_var("FEISHU_APP_ID", "tst");
        std::env::set_var("FEISHU_APP_SECRET", "tst");
        std::env::set_var("FEISHU_VERIFICATION_TOKEN", "expected");
        std::env::remove_var("FEISHU_ENCRYPT_KEY");

        let body = br#"{"type":"url_verification","challenge":"x","token":"WRONG"}"#;
        let err = handle_webhook(body, None, None, None).unwrap_err();
        assert!(err.contains("verification token"), "got: {err}");
    }

    /// SHA-256 sanity (used for X-Lark-Signature).
    #[test]
    fn sha256_hex_matches_known_vector() {
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn encrypted_payload_is_rejected() {
        let _g = crate::session::fleet_home_lock();
        isolate_creds_file("enc-rejected");
        std::env::set_var("FEISHU_APP_ID", "tst");
        std::env::set_var("FEISHU_APP_SECRET", "tst");
        std::env::remove_var("FEISHU_VERIFICATION_TOKEN");
        std::env::remove_var("FEISHU_ENCRYPT_KEY");

        let body = br#"{"encrypt":"base64stuff"}"#;
        let err = handle_webhook(body, None, None, None).unwrap_err();
        assert!(err.contains("encryption"), "got: {err}");
    }

    #[test]
    fn stored_creds_round_trip() {
        let _g = crate::session::fleet_home_lock();
        isolate_creds_file("round-trip");
        let c = StoredCreds {
            app_id: "id_x".into(),
            app_secret: "secret_x".into(),
            encrypt_key: "ek_x".into(),
            verification_token: "".into(),
        };
        set_stored_creds(c.clone()).unwrap();
        let got = get_stored_creds();
        assert_eq!(got.app_id, "id_x");
        assert_eq!(got.app_secret, "secret_x");
        assert_eq!(got.encrypt_key, "ek_x");
        assert_eq!(got.verification_token, "");
    }

    // ── Hand-written WebSocket client ────────────────────────────────────────

    fn hdr(key: &str, value: &str) -> Header {
        Header { key: key.to_string(), value: value.to_string() }
    }

    #[test]
    fn parse_service_id_from_url_query() {
        let url = "wss://example.feishu.cn/ws?device_id=abc&service_id=42&x=1";
        assert_eq!(parse_service_id(url), Some(42));
        assert_eq!(parse_service_id("wss://example.feishu.cn/ws"), None);
        assert_eq!(parse_service_id("wss://x/ws?service_id=notnum"), None);
    }

    /// A single-frame data message (no `sum`/`seq`) passes straight through.
    #[test]
    fn reassemble_single_frame_passthrough() {
        let mut buffers = HashMap::new();
        let headers = vec![hdr("type", "event")];
        let out = reassemble(&mut buffers, &headers, b"hello".to_vec());
        assert_eq!(out, Some(b"hello".to_vec()));
        assert!(buffers.is_empty(), "single frame must not leave buffer state");
    }

    /// `sum=1` is an explicit single frame, not a fragment.
    #[test]
    fn reassemble_sum_one_passthrough() {
        let mut buffers = HashMap::new();
        let headers = vec![hdr("sum", "1"), hdr("seq", "0"), hdr("message_id", "m1")];
        let out = reassemble(&mut buffers, &headers, b"abc".to_vec());
        assert_eq!(out, Some(b"abc".to_vec()));
        assert!(buffers.is_empty());
    }

    /// Multi-fragment message: only the final piece yields the joined payload,
    /// in seq order regardless of arrival order.
    #[test]
    fn reassemble_multi_fragment_joins_in_seq_order() {
        let mut buffers = HashMap::new();
        let mk = |seq: &str| {
            vec![
                hdr("type", "event"),
                hdr("sum", "3"),
                hdr("seq", seq),
                hdr("message_id", "msg-7"),
            ]
        };
        // Deliver out of order: seq 2, then 0, then 1.
        assert_eq!(reassemble(&mut buffers, &mk("2"), b"CC".to_vec()), None);
        assert_eq!(reassemble(&mut buffers, &mk("0"), b"AA".to_vec()), None);
        let out = reassemble(&mut buffers, &mk("1"), b"BB".to_vec());
        assert_eq!(out, Some(b"AABBCC".to_vec()));
        assert!(buffers.is_empty(), "completed message must clear its buffer");
    }

    /// A fragment with `sum>1` but an empty `message_id` can't be aggregated, so
    /// it degrades to single-frame passthrough rather than being dropped.
    #[test]
    fn reassemble_missing_message_id_degrades_to_passthrough() {
        let mut buffers = HashMap::new();
        let headers = vec![hdr("sum", "2"), hdr("seq", "0")];
        let out = reassemble(&mut buffers, &headers, b"x".to_vec());
        assert_eq!(out, Some(b"x".to_vec()));
        assert!(buffers.is_empty());
    }

    /// Ping frame must round-trip through prost encode/decode with method=0 and
    /// a single `type: ping` header carrying the service id.
    #[test]
    fn ping_frame_encodes_and_decodes() {
        let frame = build_ping_frame(99);
        assert_eq!(frame.method, 0);
        assert_eq!(frame.service, 99);
        assert_eq!(frame.headers.len(), 1);
        assert_eq!(frame.headers[0].key, "type");
        assert_eq!(frame.headers[0].value, "ping");
        assert!(frame.payload.is_none());

        let bytes = frame.encode_to_vec();
        let decoded = Frame::decode(&*bytes).expect("ping frame must decode");
        assert_eq!(decoded.method, 0);
        assert_eq!(decoded.service, 99);
        assert_eq!(decoded.headers[0].value, "ping");
    }

    /// The ack frame echoes the inbound headers, appends `biz_rt`, keeps the
    /// data method, and carries a `{"code":200,…}` JSON payload.
    #[test]
    fn ack_frame_echoes_headers_and_ok_payload() {
        let received = Frame {
            seq_id: 5,
            log_id: 6,
            service: 7,
            method: 1,
            headers: vec![hdr("type", "event"), hdr("message_id", "mm")],
            payload_encoding: None,
            payload_type: None,
            payload: Some(b"ignored".to_vec()),
            log_id_new: None,
        };
        let ack = build_ack_frame(&received, 12);
        assert_eq!(ack.method, 1);
        assert_eq!(ack.service, 7);
        assert_eq!(ack.seq_id, 5);
        assert_eq!(frame_header_value(&ack.headers, "type"), Some("event"));
        assert_eq!(frame_header_value(&ack.headers, "message_id"), Some("mm"));
        assert_eq!(frame_header_value(&ack.headers, "biz_rt"), Some("12"));

        let payload = ack.payload.clone().expect("ack must carry a payload");
        let v: serde_json::Value = serde_json::from_slice(&payload).unwrap();
        assert_eq!(v.get("code").and_then(|c| c.as_u64()), Some(200));

        // And it must survive a prost round-trip.
        let decoded = Frame::decode(&*ack.encode_to_vec()).unwrap();
        assert_eq!(frame_header_value(&decoded.headers, "biz_rt"), Some("12"));
    }

    /// A `card`-type data frame is skipped (no payload dispatch, no ack).
    #[test]
    fn card_data_frame_is_skipped() {
        let mut buffers = HashMap::new();
        let frame = Frame {
            seq_id: 0,
            log_id: 0,
            service: 1,
            method: 1,
            headers: vec![hdr("type", "card")],
            payload_encoding: None,
            payload_type: None,
            payload: Some(b"{}".to_vec()),
            log_id_new: None,
        };
        assert!(handle_data_frame(frame, &mut buffers).is_none());
    }
}
