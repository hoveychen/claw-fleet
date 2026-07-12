//! Mobile relay channel — connects the desktop to a `fleet-relay` instance
//! over an *outbound* WebSocket (same topology as the Feishu channel: no
//! inbound port, works behind NAT) so the mobile web app can receive decision
//! cards / session snapshots and send answers back.
//!
//! Transport frames (relay envelope, see `fleet-relay/src/frames.rs`):
//!   out:  `auth` / `msg{payload}` / `notify{title,body,tag,url}`
//!   in:   `authed` / `presence{clients}` / `msg{payload}` / `error`
//!
//! Business frames live inside `msg.payload` and are Fleet-specific:
//!   agent → client: `decision_created{kind,request}` / `decision_resolved{kind,id}`
//!                   / `sessions{sessions}` / `reply{req_id,ok,data}`
//!   client → agent: `answer{kind,id,...}` / `req{req_id,method,params}`
//!
//! Answers are written back through the same channel-agnostic
//! `write_response()` files the desktop panel and Feishu use, so the waiting
//! hook/MCP subprocess unblocks without any session-side changes.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const DEFAULT_RELAY_URL: &str = "https://fleet-relay.muveeai.com";
const CONFIG_FILE_NAME: &str = "mobile-relay.json";
/// Minimum seconds between two `sessions` snapshots pushed to clients.
const SESSIONS_THROTTLE: Duration = Duration::from_secs(2);

// ── Config ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MobileRelayConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_relay_url")]
    pub relay_url: String,
    /// Shared secret pairing this desktop with its mobile clients. Generated
    /// on first enable; embedded in the QR code as the URL fragment.
    #[serde(default)]
    pub secret: String,
}

fn default_relay_url() -> String {
    DEFAULT_RELAY_URL.to_string()
}

impl Default for MobileRelayConfig {
    fn default() -> Self {
        Self { enabled: false, relay_url: default_relay_url(), secret: String::new() }
    }
}

fn config_path() -> Option<PathBuf> {
    crate::session::get_fleet_dir().map(|d| d.join(CONFIG_FILE_NAME))
}

pub fn load_config() -> MobileRelayConfig {
    let Some(p) = config_path() else {
        return MobileRelayConfig::default();
    };
    fs::read_to_string(&p)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist the config (0600 — the secret grants decision-answering rights)
/// and poke the WS client so it reconnects with the new settings.
pub fn save_config(cfg: &MobileRelayConfig) -> std::io::Result<()> {
    let p = config_path()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "no fleet dir"))?;
    if let Some(parent) = p.parent() {
        fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(cfg)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(&p, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
    }
    CONFIG_GEN.fetch_add(1, Ordering::SeqCst);
    if cfg.enabled {
        ensure_ws_client();
    }
    Ok(())
}

/// 64 hex chars of UUIDv4 entropy (2 × 122 bits) — long enough that the
/// relay-side SHA-256 channel id cannot be brute-forced.
pub fn generate_secret() -> String {
    format!("{}{}", uuid::Uuid::new_v4().simple(), uuid::Uuid::new_v4().simple())
}

/// The URL the mobile web app opens; the secret rides in the fragment so it
/// never reaches server logs.
pub fn pairing_url(cfg: &MobileRelayConfig) -> String {
    format!("{}/#k={}", cfg.relay_url.trim_end_matches('/'), cfg.secret)
}

/// Persist a config coming from the UI. An enabled config with an empty
/// secret keeps the existing secret (or generates one on first enable) so the
/// frontend never has to round-trip the secret value.
pub fn set_config_normalized(mut cfg: MobileRelayConfig) -> Result<MobileRelayConfig, String> {
    if cfg.relay_url.trim().is_empty() {
        cfg.relay_url = default_relay_url();
    }
    if cfg.secret.is_empty() {
        let existing = load_config().secret;
        cfg.secret = if existing.is_empty() { generate_secret() } else { existing };
    }
    save_config(&cfg).map_err(|e| format!("save mobile relay config: {e}"))?;
    Ok(cfg)
}

/// Rotate the pairing secret — the old QR code stops working as soon as the
/// WS client reconnects with the new channel.
pub fn rotate_secret() -> Result<MobileRelayConfig, String> {
    let mut cfg = load_config();
    cfg.secret = generate_secret();
    save_config(&cfg).map_err(|e| format!("save mobile relay config: {e}"))?;
    Ok(cfg)
}

/// Render the pairing URL as an SVG QR code for the 「移动端」 view.
pub fn qr_svg() -> Result<String, String> {
    let cfg = load_config();
    if cfg.secret.is_empty() {
        return Err("mobile relay secret not set".into());
    }
    let code = qrcode::QrCode::new(pairing_url(&cfg).as_bytes())
        .map_err(|e| format!("qr encode: {e}"))?;
    Ok(code
        .render::<qrcode::render::svg::Color>()
        .quiet_zone(true)
        .min_dimensions(240, 240)
        .dark_color(qrcode::render::svg::Color("#000000"))
        .light_color(qrcode::render::svg::Color("#ffffff"))
        .build())
}

// ── Status (for the desktop 「移动端」 view) ─────────────────────────────────

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MobileRelayStatus {
    pub enabled: bool,
    pub connected: bool,
    pub clients: usize,
    pub relay_url: String,
    pub secret_set: bool,
}

/// Cheap connectivity probe so callers can skip serialization work while the
/// relay is down.
pub fn is_connected() -> bool {
    CONNECTED.load(Ordering::SeqCst)
}

/// Short single-line preview for a push notification body.
pub fn notify_preview(text: &str) -> String {
    let first_line = text.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    let mut s: String = first_line.trim().chars().take(80).collect();
    if first_line.trim().chars().count() > 80 {
        s.push('…');
    }
    s
}

/// Workspace label for notification titles (falls back when the request
/// predates session-display resolution).
pub fn notify_workspace(workspace_name: &str) -> &str {
    if workspace_name.is_empty() { "Fleet" } else { workspace_name }
}

pub fn status() -> MobileRelayStatus {
    let cfg = load_config();
    MobileRelayStatus {
        enabled: cfg.enabled,
        connected: CONNECTED.load(Ordering::SeqCst),
        clients: CLIENTS.load(Ordering::SeqCst),
        relay_url: cfg.relay_url,
        secret_set: !cfg.secret.is_empty(),
    }
}

// ── Outbound publishing ──────────────────────────────────────────────────────

static WS_STARTED: AtomicBool = AtomicBool::new(false);
static CONNECTED: AtomicBool = AtomicBool::new(false);
static CLIENTS: AtomicUsize = AtomicUsize::new(0);
/// Bumped on every config save; the connect loop drops the current socket
/// when it observes a stale generation (secret rotation, disable, URL edit).
static CONFIG_GEN: AtomicU64 = AtomicU64::new(0);
static OUT_TX: Mutex<Option<tokio::sync::mpsc::UnboundedSender<String>>> = Mutex::new(None);
static SESSIONS_LAST_SENT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

fn send_raw(frame: String) {
    if let Some(tx) = OUT_TX.lock().unwrap().as_ref() {
        let _ = tx.send(frame);
    }
}

fn build_msg_frame(payload: Value) -> String {
    json!({ "type": "msg", "payload": payload }).to_string()
}

fn build_notify_frame(title: &str, body: &str, tag: &str) -> String {
    json!({ "type": "notify", "title": title, "body": body, "tag": tag, "url": "/" }).to_string()
}

pub fn build_decision_created_payload(kind: &str, request: Value) -> Value {
    json!({ "event": "decision_created", "kind": kind, "request": request })
}

pub fn build_decision_resolved_payload(kind: &str, id: &str) -> Value {
    json!({ "event": "decision_resolved", "kind": kind, "id": id })
}

/// Publish a new pending decision to connected mobile clients and fan out a
/// Web Push notification (push reaches phones even with the browser closed,
/// so it is sent regardless of live client presence).
pub fn publish_decision_created(kind: &str, request: Value, notify_title: &str, notify_body: &str) {
    let tag = request
        .get("id")
        .and_then(Value::as_str)
        .map(|id| format!("{kind}:{id}"))
        .unwrap_or_else(|| kind.to_string());
    send_raw(build_msg_frame(build_decision_created_payload(kind, request)));
    send_raw(build_notify_frame(notify_title, notify_body, &tag));
}

/// Tell mobile clients a decision disappeared (answered elsewhere / timed out).
pub fn publish_decision_resolved(kind: &str, id: &str) {
    send_raw(build_msg_frame(build_decision_resolved_payload(kind, id)));
}

/// Push a sessions snapshot. Skipped when no client is online (Web Push is
/// pointless for passive data) and throttled to one per 2s.
pub fn publish_sessions(sessions: &Value) {
    if !CONNECTED.load(Ordering::SeqCst) || CLIENTS.load(Ordering::SeqCst) == 0 {
        return;
    }
    {
        let mut last = SESSIONS_LAST_SENT.lock().unwrap();
        if let Some(t) = *last {
            if t.elapsed() < SESSIONS_THROTTLE {
                return;
            }
        }
        *last = Some(std::time::Instant::now());
    }
    send_raw(build_msg_frame(json!({ "event": "sessions", "sessions": sessions })));
}

// ── Inbound: answers and requests from mobile clients ────────────────────────

/// Handle one business payload sent by a mobile client. Returns an optional
/// reply payload to send back. Implemented in this module so the write-back
/// path stays unit-testable without sockets.
pub fn handle_client_payload(payload: &Value) -> Option<Value> {
    match payload.get("event").and_then(Value::as_str) {
        Some("answer") => {
            if let Err(e) = handle_answer(payload) {
                crate::log_debug(&format!("[mobile-relay] bad answer: {e}"));
            }
            None
        }
        Some("req") => Some(handle_request(payload)),
        other => {
            crate::log_debug(&format!("[mobile-relay] unknown client event: {other:?}"));
            None
        }
    }
}

/// Route a mobile answer into the same `write_response()` files the desktop
/// panel and Feishu write — the waiting hook/MCP subprocess polls these and
/// unblocks (see `feishu.rs::handle_card_action` for the sibling path).
fn handle_answer(payload: &Value) -> Result<(), String> {
    let kind = payload.get("kind").and_then(Value::as_str).ok_or("missing kind")?;
    let id = payload
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("missing id")?
        .to_string();
    let str_field = |key: &str| -> Option<String> {
        payload.get(key).and_then(Value::as_str).map(str::to_string).filter(|s| !s.is_empty())
    };
    match kind {
        "guard" => {
            let allow = payload.get("allow").and_then(Value::as_bool).ok_or("missing allow")?;
            crate::guard::write_response(&crate::guard::GuardResponse {
                id,
                decision: if allow {
                    crate::guard::GuardDecision::Allow
                } else {
                    crate::guard::GuardDecision::Block
                },
                reason: str_field("reason"),
            })
        }
        "elicitation" => {
            let answers: std::collections::HashMap<String, String> = payload
                .get("answers")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| format!("bad answers: {e}"))?
                .unwrap_or_default();
            crate::elicitation::write_response(&crate::elicitation::ElicitationResponse {
                id,
                declined: payload.get("declined").and_then(Value::as_bool).unwrap_or(false),
                answers,
            })
        }
        "fleet-ask" => {
            let answers: std::collections::BTreeMap<String, String> = payload
                .get("answers")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| format!("bad answers: {e}"))?
                .unwrap_or_default();
            crate::mcp_ipc::write_response(&crate::mcp_ipc::FleetAskResponse {
                id,
                answers,
                cancelled: payload.get("cancelled").and_then(Value::as_bool).unwrap_or(false),
            })
        }
        "plan-approval" => {
            let decision = str_field("decision").ok_or("missing decision")?;
            crate::plan_approval::write_response(&crate::plan_approval::PlanApprovalResponse {
                id,
                decision,
                edited_plan: str_field("editedPlan"),
                feedback: str_field("feedback"),
            })
        }
        "permission-prompt" => {
            let allow = payload.get("allow").and_then(Value::as_bool).ok_or("missing allow")?;
            crate::permission_prompt_ipc::write_response(
                &crate::permission_prompt_ipc::PermissionPromptResponse {
                    id,
                    decision: if allow {
                        crate::permission_prompt_ipc::PermissionPromptDecision::Allow
                    } else {
                        crate::permission_prompt_ipc::PermissionPromptDecision::Deny
                    },
                    reason: str_field("reason"),
                },
            )
        }
        other => Err(format!("unknown decision kind: {other}")),
    }
}

/// Serve a data request from a mobile client. Always returns a `reply`
/// payload carrying the same `req_id` so the client can match it up.
fn handle_request(payload: &Value) -> Value {
    let req_id = payload.get("req_id").cloned().unwrap_or(Value::Null);
    let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
    let params = payload.get("params").cloned().unwrap_or(Value::Null);
    match serve_request(method, &params) {
        Ok(data) => json!({ "event": "reply", "req_id": req_id, "ok": true, "data": data }),
        Err(e) => json!({ "event": "reply", "req_id": req_id, "ok": false, "error": e }),
    }
}

fn serve_request(method: &str, params: &Value) -> Result<Value, String> {
    match method {
        // Same aggregation as `LocalBackend::list_pending_decisions`, minus
        // the session-cache display enrichment — the mobile client resolves
        // workspace/title labels from its own `sessions` snapshot.
        "pending_snapshot" => {
            let read_all = |ids: Vec<String>, read: &dyn Fn(&str) -> Option<Value>| -> Vec<Value> {
                ids.iter().filter_map(|id| read(id)).collect()
            };
            Ok(json!({
                "guard": read_all(crate::guard::list_pending_requests(), &|id| {
                    crate::guard::read_request(id).and_then(|r| serde_json::to_value(r).ok())
                }),
                "elicitation": read_all(crate::elicitation::list_pending_requests(), &|id| {
                    crate::elicitation::read_request(id).and_then(|r| serde_json::to_value(r).ok())
                }),
                "fleetAsk": read_all(crate::mcp_ipc::list_pending_requests(), &|id| {
                    crate::mcp_ipc::read_request(id).and_then(|r| serde_json::to_value(r).ok())
                }),
                "planApproval": read_all(crate::plan_approval::list_pending_requests(), &|id| {
                    crate::plan_approval::read_request(id).and_then(|r| serde_json::to_value(r).ok())
                }),
                "permissionPrompt": read_all(
                    crate::permission_prompt_ipc::list_pending_requests(),
                    &|id| {
                        crate::permission_prompt_ipc::read_request(id)
                            .and_then(|r| serde_json::to_value(r).ok())
                    },
                ),
            }))
        }
        "task_plans" => {
            let workspace = params
                .get("workspacePath")
                .and_then(Value::as_str)
                .ok_or("missing workspacePath")?;
            let session_id = params.get("sessionId").and_then(Value::as_str);
            let plans =
                crate::prd_tasks::list_workspace_task_plans(std::path::Path::new(workspace), session_id);
            serde_json::to_value(plans).map_err(|e| e.to_string())
        }
        "session_decisions" => {
            let session_id = params
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or("missing sessionId")?;
            let jsonl = params.get("jsonlPath").and_then(Value::as_str).map(std::path::Path::new);
            let records =
                crate::decision_history::list_session_records_with_jsonl(session_id, jsonl);
            serde_json::to_value(records).map_err(|e| e.to_string())
        }
        "decision_asset" => {
            use base64::Engine as _;
            let id = params.get("id").and_then(Value::as_str).ok_or("missing id")?;
            let qidx = params.get("qidx").and_then(Value::as_str).ok_or("missing qidx")?;
            let rel = params.get("rel").and_then(Value::as_str).ok_or("missing rel")?;
            let asset = crate::mcp_ipc::read_decision_asset(id, qidx, rel)?;
            const MAX_ASSET_BYTES: usize = 2 * 1024 * 1024;
            if asset.bytes.len() > MAX_ASSET_BYTES {
                return Err(format!("asset too large: {} bytes", asset.bytes.len()));
            }
            Ok(json!({
                "mime": asset.mime,
                "base64": base64::engine::general_purpose::STANDARD.encode(&asset.bytes),
            }))
        }
        other => Err(format!("unknown method: {other}")),
    }
}

// ── WebSocket client (mirrors feishu.rs's ensure_ws_client/ws_run_loop) ─────

/// Convert the configured https relay URL into the ws endpoint.
fn ws_url(relay_url: &str) -> String {
    let base = relay_url.trim_end_matches('/');
    let ws_base = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws_base}/ws")
}

/// Ensure the outbound relay WebSocket is running. Idempotent; no-op when
/// the feature is disabled or no secret is set.
pub fn ensure_ws_client() {
    let cfg = load_config();
    if !cfg.enabled || cfg.secret.is_empty() {
        return;
    }
    if WS_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("mobile-relay-ws".into())
        .spawn(|| {
            match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(rt) => rt.block_on(ws_run_loop()),
                Err(e) => {
                    eprintln!("mobile-relay ws: runtime build failed: {e}");
                    WS_STARTED.store(false, Ordering::SeqCst);
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("mobile-relay ws: spawn failed: {e}");
        WS_STARTED.store(false, Ordering::SeqCst);
    }
}

async fn ws_run_loop() {
    loop {
        let cfg = load_config();
        if !cfg.enabled || cfg.secret.is_empty() {
            WS_STARTED.store(false, Ordering::SeqCst);
            return;
        }
        let gen = CONFIG_GEN.load(Ordering::SeqCst);
        match ws_connect_once(&cfg, gen).await {
            Ok(()) => crate::log_debug("[mobile-relay] disconnected; reconnecting in 5s"),
            Err(e) => crate::log_debug(&format!("[mobile-relay] error: {e}")),
        }
        CONNECTED.store(false, Ordering::SeqCst);
        CLIENTS.store(0, Ordering::SeqCst);
        *OUT_TX.lock().unwrap() = None;
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

async fn ws_connect_once(cfg: &MobileRelayConfig, gen: u64) -> Result<(), String> {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let url = ws_url(&cfg.relay_url);
    let (ws, _) = tokio_tungstenite::connect_async(&url)
        .await
        .map_err(|e| format!("connect {url}: {e}"))?;
    let (mut sink, mut stream) = ws.split();

    // auth as agent
    let auth = json!({ "type": "auth", "role": "agent", "secret": cfg.secret }).to_string();
    sink.send(Message::Text(auth.into()))
        .await
        .map_err(|e| format!("auth send: {e}"))?;

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
    *OUT_TX.lock().unwrap() = Some(tx);

    let mut ping = tokio::time::interval(Duration::from_secs(30));
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut config_check = tokio::time::interval(Duration::from_secs(5));
    let mut last_inbound = std::time::Instant::now();

    loop {
        tokio::select! {
            inbound = stream.next() => {
                let Some(Ok(msg)) = inbound else {
                    return Ok(()); // closed / errored → reconnect
                };
                last_inbound = std::time::Instant::now();
                let Message::Text(text) = msg else { continue };
                let Ok(frame) = serde_json::from_str::<Value>(&text) else { continue };
                match frame.get("type").and_then(Value::as_str) {
                    Some("authed") => {
                        CONNECTED.store(true, Ordering::SeqCst);
                        if let Some(n) = frame.get("clients").and_then(Value::as_u64) {
                            CLIENTS.store(n as usize, Ordering::SeqCst);
                        }
                        crate::log_debug("[mobile-relay] connected");
                    }
                    Some("presence") => {
                        if let Some(n) = frame.get("clients").and_then(Value::as_u64) {
                            CLIENTS.store(n as usize, Ordering::SeqCst);
                        }
                    }
                    Some("msg") => {
                        if let Some(payload) = frame.get("payload") {
                            // Blocking file IO (write_response / pending scans)
                            // must not run on the ws runtime thread.
                            let payload = payload.clone();
                            let reply = tokio::task::spawn_blocking(move || {
                                handle_client_payload(&payload)
                            })
                            .await
                            .ok()
                            .flatten();
                            if let Some(reply) = reply {
                                send_raw(build_msg_frame(reply));
                            }
                        }
                    }
                    Some("error") => {
                        return Err(format!("relay error: {frame}"));
                    }
                    _ => {}
                }
            }
            outbound = rx.recv() => {
                let Some(s) = outbound else { return Ok(()) };
                sink.send(Message::Text(s.into())).await.map_err(|e| format!("send: {e}"))?;
            }
            _ = ping.tick() => {
                if sink.send(Message::Ping(Vec::new().into())).await.is_err() {
                    return Ok(());
                }
                if last_inbound.elapsed() > Duration::from_secs(90) {
                    return Err("heartbeat timeout".into());
                }
            }
            _ = config_check.tick() => {
                if CONFIG_GEN.load(Ordering::SeqCst) != gen {
                    return Err("config changed; reconnecting".into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::fleet_home_lock;

    fn with_temp_home<F: FnOnce()>(f: F) {
        let _guard = fleet_home_lock();
        let tmp = std::env::temp_dir().join(format!(
            "fleet-mobile-relay-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let _ = fs::create_dir_all(&tmp);
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by fleet_home_lock.
        unsafe { std::env::set_var("FLEET_HOME", &tmp) };
        f();
        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn config_roundtrip_and_default() {
        with_temp_home(|| {
            let cfg = load_config();
            assert!(!cfg.enabled);
            assert_eq!(cfg.relay_url, DEFAULT_RELAY_URL);
            assert!(cfg.secret.is_empty());

            let cfg = MobileRelayConfig {
                enabled: true,
                relay_url: "https://relay.example.com".into(),
                secret: generate_secret(),
            };
            save_config(&cfg).unwrap();
            let loaded = load_config();
            assert_eq!(loaded, cfg);

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = fs::metadata(config_path().unwrap()).unwrap().permissions().mode();
                assert_eq!(mode & 0o777, 0o600, "secret file must be 0600");
            }
        });
    }

    #[test]
    fn secret_generation_and_pairing_url() {
        let a = generate_secret();
        let b = generate_secret();
        assert_eq!(a.len(), 64);
        assert_ne!(a, b);
        let cfg = MobileRelayConfig {
            enabled: true,
            relay_url: "https://relay.example.com/".into(),
            secret: "s".repeat(64),
        };
        assert_eq!(
            pairing_url(&cfg),
            format!("https://relay.example.com/#k={}", "s".repeat(64))
        );
    }

    #[test]
    fn ws_url_derivation() {
        assert_eq!(ws_url("https://relay.example.com"), "wss://relay.example.com/ws");
        assert_eq!(ws_url("https://relay.example.com/"), "wss://relay.example.com/ws");
        assert_eq!(ws_url("http://127.0.0.1:18080"), "ws://127.0.0.1:18080/ws");
    }

    fn fleet_subdir(name: &str) -> PathBuf {
        let d = crate::session::get_fleet_dir().unwrap().join(name);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn read_json(p: &std::path::Path) -> Value {
        let raw = fs::read_to_string(p)
            .unwrap_or_else(|e| panic!("expected response file at {}: {e}", p.display()));
        serde_json::from_str(&raw).unwrap()
    }

    #[test]
    fn answer_guard_writes_response_file() {
        with_temp_home(|| {
            let dir = fleet_subdir("guard");
            handle_client_payload(&json!({
                "event": "answer", "kind": "guard", "id": "g1",
                "allow": false, "reason": "危险命令"
            }));
            let v = read_json(&dir.join("g1.response.json"));
            assert_eq!(v["decision"], "block");
            assert_eq!(v["reason"], "危险命令");

            handle_client_payload(&json!({
                "event": "answer", "kind": "guard", "id": "g2", "allow": true
            }));
            let v = read_json(&dir.join("g2.response.json"));
            assert_eq!(v["decision"], "allow");
        });
    }

    #[test]
    fn answer_elicitation_writes_response_file() {
        with_temp_home(|| {
            let dir = fleet_subdir("elicitation");
            handle_client_payload(&json!({
                "event": "answer", "kind": "elicitation", "id": "e1",
                "declined": false,
                "answers": { "选哪个？": "方案 A" }
            }));
            let v = read_json(&dir.join("e1.response.json"));
            assert_eq!(v["declined"], false);
            assert_eq!(v["answers"]["选哪个？"], "方案 A");
        });
    }

    #[test]
    fn answer_fleet_ask_writes_response_file() {
        with_temp_home(|| {
            let dir = fleet_subdir("fleet-ask");
            handle_client_payload(&json!({
                "event": "answer", "kind": "fleet-ask", "id": "f1",
                "cancelled": false,
                "answers": { "q": "opt", "field_a": "42" }
            }));
            let v = read_json(&dir.join("f1.response.json"));
            assert_eq!(v["cancelled"], false);
            assert_eq!(v["answers"]["field_a"], "42");
        });
    }

    #[test]
    fn answer_plan_approval_writes_response_file() {
        with_temp_home(|| {
            let dir = fleet_subdir("plan-approval");
            handle_client_payload(&json!({
                "event": "answer", "kind": "plan-approval", "id": "p1",
                "decision": "reject", "feedback": "改一下再来"
            }));
            let v = read_json(&dir.join("p1.response.json"));
            assert_eq!(v["decision"], "reject");
            assert_eq!(v["feedback"], "改一下再来");
        });
    }

    #[test]
    fn answer_permission_prompt_writes_response_file() {
        with_temp_home(|| {
            let dir = fleet_subdir("permission-prompt");
            handle_client_payload(&json!({
                "event": "answer", "kind": "permission-prompt", "id": "pp1",
                "allow": true
            }));
            let v = read_json(&dir.join("pp1.response.json"));
            assert_eq!(v["decision"], "allow");
        });
    }

    #[test]
    fn request_pending_snapshot_returns_reply() {
        with_temp_home(|| {
            let dir = fleet_subdir("guard");
            fs::write(
                dir.join("g9.json"),
                serde_json::to_string(&json!({
                    "id": "g9", "sessionId": "s1", "workspaceName": "demo",
                    "toolName": "Bash", "command": "rm -rf /tmp/x",
                    "commandSummary": "rm", "riskTags": [], "timestamp": "2026-07-12T10:00:00Z"
                }))
                .unwrap(),
            )
            .unwrap();
            let reply = handle_client_payload(&json!({
                "event": "req", "req_id": "r1", "method": "pending_snapshot"
            }))
            .expect("pending_snapshot must produce a reply");
            assert_eq!(reply["event"], "reply");
            assert_eq!(reply["req_id"], "r1");
            assert_eq!(reply["ok"], true);
            let guards = reply["data"]["guard"].as_array().expect("guard list");
            assert_eq!(guards.len(), 1);
            assert_eq!(guards[0]["id"], "g9");
        });
    }

    #[test]
    fn decision_frames_shape() {
        let payload = build_decision_created_payload("guard", serde_json::json!({"id": "g1"}));
        assert_eq!(payload["event"], "decision_created");
        assert_eq!(payload["kind"], "guard");
        assert_eq!(payload["request"]["id"], "g1");

        let payload = build_decision_resolved_payload("elicitation", "e1");
        assert_eq!(payload["event"], "decision_resolved");
        assert_eq!(payload["id"], "e1");

        let frame: Value = serde_json::from_str(&build_notify_frame("t", "b", "guard:g1")).unwrap();
        assert_eq!(frame["type"], "notify");
        assert_eq!(frame["tag"], "guard:g1");
    }
}
