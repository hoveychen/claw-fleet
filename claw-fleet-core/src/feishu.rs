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
//!   Card 2.0 send/update/urgent_app, decision_id → message_id map.
//! - **TODO (separate slice):** webhook `/webhook/feishu` signature
//!   verification + `card.action.trigger` dispatch.
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
    save_creds_file(&c)
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
    ResolvedBeforeSent { kind: DecisionKind, summary: String },
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
            };
            crate::guard::write_response(&resp)?;
            let summary = if allow { "✅ Allow (Feishu)" } else { "🚫 Block (Feishu)" };
            let _ = notify_decision_resolved(decision_id, DecisionKind::Guard, summary);
        }
        "elicitation" => {
            let label = value
                .get("answer_label")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let question = value
                .get("question")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            let mut answers = HashMap::new();
            if !question.is_empty() {
                answers.insert(question, label.clone());
            }
            let resp = crate::elicitation::ElicitationResponse {
                id: decision_id.to_string(),
                declined: false,
                answers,
            };
            crate::elicitation::write_response(&resp)?;
            let summary = format!("✅ {label} (Feishu)");
            let _ = notify_decision_resolved(decision_id, DecisionKind::Elicitation, &summary);
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
            crate::plan_approval::write_response(&resp)?;
            let summary = if decision == "approve" {
                "✅ Approved (Feishu)"
            } else {
                "🚫 Rejected (Feishu)"
            };
            let _ = notify_decision_resolved(decision_id, DecisionKind::PlanApproval, summary);
        }
        other => return Err(format!("unknown card action: {other}")),
    }
    Ok(b"{}".to_vec())
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

#[derive(Serialize, Debug, Clone)]
pub struct ElicitationCard {
    pub workspace: String,
    pub question: String,
    pub options: Vec<ElicitationOptionCard>,
    pub multi_select: bool,
    pub allow_other: bool,
    pub step: Option<(u32, u32)>,
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
    pub fn to_card_json(&self) -> serde_json::Value {
        let mut body = String::new();
        body.push_str(&format!("**工作区:** {}\n\n", escape_md(&self.workspace)));
        if let Some((cur, total)) = self.step {
            body.push_str(&format!("_问题 {cur}/{total}_\n\n"));
        }
        body.push_str(&format!("**{}**", escape_md(&self.question)));

        let mut elements: Vec<serde_json::Value> = vec![
            serde_json::json!({ "tag": "markdown", "content": body }),
        ];

        // Multi-select isn't supported via simple callback buttons (would
        // need a form + submit button).  Render a notice and let the
        // desktop UI handle it.
        if self.multi_select {
            elements.push(serde_json::json!({
                "tag": "markdown",
                "content": "_多选问题请在 Fleet 桌面端选择。_",
            }));
            return wrap_card("Decision needed · 多选", "blue", elements);
        }

        for opt in &self.options {
            let value = serde_json::json!({
                "action": "elicitation",
                "decision_id": self.decision_id,
                "question": self.question,
                "answer_label": opt.label,
            });
            let label = if let Some(desc) = &opt.description {
                format!("{} — {}", opt.label, desc)
            } else {
                opt.label.clone()
            };
            elements.push(callback_button(&label, "default", value));
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
                    Some(DecisionState::ResolvedBeforeSent { kind: rkind, summary }) => {
                        Some((rkind, summary))
                    }
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

            if let Some((rkind, summary)) = pending_resolve {
                let card = resolved_card(rkind, &summary);
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

/// Flips the previously-sent card into a read-only "已处理" state.  If
/// the send is still in flight, leaves a marker so the send-thread
/// applies the update once it has a `message_id`.  No-op if no card was
/// ever sent (e.g. not connected).
pub fn notify_decision_resolved(
    decision_id: &str,
    kind: DecisionKind,
    summary: &str,
) -> Result<(), String> {
    let message_id = {
        let mut map = bridge().decision_states.lock().unwrap();
        match map.get(decision_id).cloned() {
            Some(DecisionState::Sent { message_id }) => {
                map.remove(decision_id);
                Some(message_id)
            }
            Some(DecisionState::Sending) => {
                map.insert(
                    decision_id.to_string(),
                    DecisionState::ResolvedBeforeSent {
                        kind,
                        summary: summary.to_string(),
                    },
                );
                None
            }
            // Already-marked or never-existed both no-op.
            _ => None,
        }
    };
    let Some(message_id) = message_id else {
        return Ok(());
    };
    let card = resolved_card(kind, summary);
    update_card(&message_id, &card)
}

fn resolved_card(kind: DecisionKind, summary: &str) -> serde_json::Value {
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
    fn elicitation_card_emits_one_button_per_option() {
        let card = ElicitationCard {
            workspace: "demo".into(),
            question: "Which?".into(),
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
            allow_other: false,
            step: None,
            decision_id: "e-9".into(),
        }
        .to_card_json();
        let buttons: Vec<&serde_json::Value> = card
            .pointer("/body/elements")
            .and_then(|e| e.as_array())
            .unwrap()
            .iter()
            .filter(|el| el.get("tag").and_then(|t| t.as_str()) == Some("button"))
            .collect();
        assert_eq!(buttons.len(), 2);
        for btn in &buttons {
            let v = btn.pointer("/behaviors/0/value").unwrap();
            assert_eq!(v.get("action").and_then(|x| x.as_str()), Some("elicitation"));
            assert_eq!(v.get("decision_id").and_then(|x| x.as_str()), Some("e-9"));
            assert_eq!(v.get("question").and_then(|x| x.as_str()), Some("Which?"));
            assert!(v.get("answer_label").and_then(|x| x.as_str()).is_some());
        }
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
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    fn isolate_creds_file(slot: &str) {
        let dir = std::env::temp_dir().join(format!("claw-fleet-feishu-{slot}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("FLEET_HOME", &dir);
    }

    #[test]
    fn url_verification_echoes_challenge() {
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
        let _g = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
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
}
