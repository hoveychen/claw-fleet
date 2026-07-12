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
//!                   / `client_hello{clientId,label,platform,pushSubscribed}` (presence)
//!                   / `client_bye{clientId}`
//!
//! Answers are written back through the same channel-agnostic
//! `write_response()` files the desktop panel and Feishu use, so the waiting
//! hook/MCP subprocess unblocks without any session-side changes.

use std::collections::HashMap;
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
/// Raw-byte cap for `upload_attachment`. Base64 inflates ×4/3, so 10 MiB stays
/// well under the relay's 16 MiB WS frame / 64 MiB message limits (axum
/// defaults) while covering phone camera photos.
pub const MAX_UPLOAD_BYTES: u64 = 10 * 1024 * 1024;

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
    /// Currently-connected mobile clients that announced themselves via
    /// `client_hello`. Best-effort: pruned on a heartbeat timeout (the relay
    /// only reports a client *count*, never which client left). `#[serde(default)]`
    /// keeps a RemoteBackend probe against an older `fleet serve` deserializable.
    #[serde(default)]
    pub devices: Vec<MobileClientInfo>,
}

/// One connected mobile client, as reported by its periodic `client_hello`.
/// The web app cannot read a real device name, so `label`/`platform` are an
/// honest UA-derived best guess (see mobile-web `deviceLabel.ts`).
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct MobileClientInfo {
    pub client_id: String,
    pub label: String,
    pub platform: String,
    pub push_subscribed: bool,
    pub connected_at_ms: u64,
    pub last_seen_ms: u64,
}

/// Cheap connectivity probe so callers can skip serialization work while the
/// relay is down.
pub fn is_connected() -> bool {
    CONNECTED.load(Ordering::SeqCst)
}

/// Number of mobile clients currently online on the channel.
pub fn client_count() -> usize {
    CLIENTS.load(Ordering::SeqCst)
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
        devices: live_devices(),
    }
}

// ── Connected-client registry ─────────────────────────────────────────────────
//
// The relay only reports a client *count* (`presence{clients}`) and never says
// which connection dropped, so we can't get per-device presence from the
// transport. Instead each mobile client periodically announces itself via a
// `client_hello` business frame; we keep the latest report per `clientId` and
// treat a client as gone once its `last_seen_ms` is older than
// `CLIENT_STALE_MS` (a multiple of the client's ~15s heartbeat).

/// Drop a client this long after its last `client_hello` (heartbeat is ~15s).
const CLIENT_STALE_MS: u64 = 40_000;

static CLIENTS_REGISTRY: Mutex<Option<HashMap<String, MobileClientInfo>>> = Mutex::new(None);

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Record (or refresh) a client from its `client_hello` payload. First sighting
/// stamps `connected_at_ms`; later ones only bump `last_seen_ms` and metadata.
fn upsert_client(payload: &Value) {
    let Some(client_id) = payload.get("clientId").and_then(Value::as_str).filter(|s| !s.is_empty())
    else {
        return;
    };
    let now = now_ms();
    let label = payload.get("label").and_then(Value::as_str).unwrap_or("").to_string();
    let platform = payload.get("platform").and_then(Value::as_str).unwrap_or("").to_string();
    let push_subscribed =
        payload.get("pushSubscribed").and_then(Value::as_bool).unwrap_or(false);
    let mut guard = CLIENTS_REGISTRY.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let entry = map.entry(client_id.to_string()).or_insert_with(|| MobileClientInfo {
        client_id: client_id.to_string(),
        label: label.clone(),
        platform: platform.clone(),
        push_subscribed,
        connected_at_ms: now,
        last_seen_ms: now,
    });
    entry.label = label;
    entry.platform = platform;
    entry.push_subscribed = push_subscribed;
    entry.last_seen_ms = now;
}

/// Forget a client that said goodbye (best-effort — mobile `pagehide` is
/// unreliable, so `CLIENT_STALE_MS` pruning is the real backstop).
fn remove_client(payload: &Value) {
    let Some(client_id) = payload.get("clientId").and_then(Value::as_str) else {
        return;
    };
    if let Some(map) = CLIENTS_REGISTRY.lock().unwrap().as_mut() {
        map.remove(client_id);
    }
}

/// Forget every client (relay reported zero clients, or we reconnected).
fn clear_clients() {
    if let Some(map) = CLIENTS_REGISTRY.lock().unwrap().as_mut() {
        map.clear();
    }
}

/// Prune stale entries and return the live devices, most-recently-seen first.
fn live_devices() -> Vec<MobileClientInfo> {
    let now = now_ms();
    let mut guard = CLIENTS_REGISTRY.lock().unwrap();
    let Some(map) = guard.as_mut() else {
        return Vec::new();
    };
    map.retain(|_, info| now.saturating_sub(info.last_seen_ms) <= CLIENT_STALE_MS);
    let mut list: Vec<MobileClientInfo> = map.values().cloned().collect();
    list.sort_by(|a, b| b.last_seen_ms.cmp(&a.last_seen_ms));
    list
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

/// Fields the mobile client actually reads (mobile-web/src/types.ts
/// SessionInfo) — everything else is desktop-only weight. A full snapshot of
/// a long-lived install measured 535 KB; the slim list is what keeps the
/// 2s push affordable on a phone connection.
const SNAPSHOT_FIELDS: &[&str] = &[
    "id",
    "workspacePath",
    "workspaceName",
    "aiTitle",
    "slug",
    "status",
    "isSubagent",
    "lastMessagePreview",
    "lastActivityMs",
    "createdAtMs",
    "jsonlPath",
    "model",
    "agentSource",
    "contextPercent",
    "totalCostUsd",
    "todos",
    "taskPlan",
    "pid",
    "pidPrecise",
    "entrypoint",
    "userMark",
    "lastReadMs",
    "procAlive",
    // Relay-chain position (hop/chainLen) — the mobile task row shows the same
    // handoff chip the desktop launchpad row does. Small object; enrich sets it
    // only for sessions on a chain, so it's absent (skipped) for the rest.
    "handoff",
];

/// Most-recently-active sessions kept in the mobile snapshot.
const SNAPSHOT_MAX_SESSIONS: usize = 120;
/// `lastMessagePreview` cap (chars) — the list row clamps to two lines anyway.
const SNAPSHOT_PREVIEW_CHARS: usize = 160;

/// Slim a full desktop sessions snapshot down to what the mobile client
/// renders: drop subagents, keep the most recently active
/// [`SNAPSHOT_MAX_SESSIONS`], whitelist fields, truncate previews.
pub fn slim_sessions_snapshot(sessions: &Value) -> Value {
    let Some(list) = sessions.as_array() else {
        return Value::Array(Vec::new());
    };
    let mut kept: Vec<&Value> = list
        .iter()
        .filter(|s| !s.get("isSubagent").and_then(Value::as_bool).unwrap_or(false))
        .collect();
    kept.sort_by_key(|s| {
        std::cmp::Reverse(s.get("lastActivityMs").and_then(Value::as_u64).unwrap_or(0))
    });
    kept.truncate(SNAPSHOT_MAX_SESSIONS);

    let slimmed: Vec<Value> = kept
        .into_iter()
        .map(|s| {
            let mut out = serde_json::Map::new();
            for &key in SNAPSHOT_FIELDS {
                let Some(v) = s.get(key) else { continue };
                if v.is_null() {
                    continue;
                }
                if key == "lastMessagePreview" {
                    if let Some(text) = v.as_str() {
                        if text.chars().count() > SNAPSHOT_PREVIEW_CHARS {
                            let cut: String =
                                text.chars().take(SNAPSHOT_PREVIEW_CHARS).collect();
                            out.insert(key.to_string(), Value::String(format!("{cut}…")));
                            continue;
                        }
                    }
                }
                out.insert(key.to_string(), v.clone());
            }
            Value::Object(out)
        })
        .collect();
    Value::Array(slimmed)
}

/// Hash of the last snapshot actually sent; unchanged data is not re-pushed.
/// Reset on client presence changes (see `ws_connect_once`) so a phone that
/// just connected still gets the current state on the next scan tick.
static SESSIONS_LAST_HASH: AtomicU64 = AtomicU64::new(0);

fn snapshot_hash(v: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.to_string().hash(&mut h);
    // Never collide with the "nothing sent yet" sentinel 0.
    h.finish() | 1
}

/// Push a sessions snapshot. Skipped when no client is online (Web Push is
/// pointless for passive data), throttled to one per 2s, slimmed to the
/// mobile field set, and deduplicated against the last pushed content.
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
    let slim = slim_sessions_snapshot(sessions);
    let hash = snapshot_hash(&slim);
    if SESSIONS_LAST_HASH.swap(hash, Ordering::SeqCst) == hash {
        return; // nothing changed since the last push
    }
    send_raw(build_msg_frame(json!({ "event": "sessions", "sessions": slim })));
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
        // Presence announcements — kept in a local registry surfaced via
        // `status().devices`; no reply needed.
        Some("client_hello") => {
            upsert_client(payload);
            None
        }
        Some("client_bye") => {
            remove_client(payload);
            None
        }
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
            // "Always allow": persist the rule BEFORE writing the response file
            // so a subsequent `fleet guard` for the same prefix already sees it
            // (same order as the desktop panel and `/guard/respond`). Only
            // honoured on allow — "Block + always allow" would be nonsensical.
            if allow {
                if let Some(rule) = payload
                    .get("alwaysAllow")
                    .cloned()
                    .and_then(|v| serde_json::from_value::<crate::guard::GuardAlwaysAllow>(v).ok())
                {
                    if !rule.prefix.trim().is_empty() {
                        crate::audit::add_guard_allow_rule(rule.prefix, rule.source_tag);
                    }
                }
            }
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
        "a2ui-render" => {
            let action_context: std::collections::BTreeMap<String, String> = payload
                .get("actionContext")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| format!("bad actionContext: {e}"))?
                .unwrap_or_default();
            crate::mcp_a2ui_ipc::write_response(&crate::mcp_a2ui_ipc::A2uiRenderResponse {
                id,
                action_name: str_field("actionName"),
                action_context,
                cancelled: payload.get("cancelled").and_then(Value::as_bool).unwrap_or(false),
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
                "a2uiRender": read_all(crate::mcp_a2ui_ipc::list_pending_requests(), &|id| {
                    crate::mcp_a2ui_ipc::read_request(id).and_then(|r| serde_json::to_value(r).ok())
                }),
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
        // Last `n` transcript messages — the mobile SessionDetail polls this
        // (same model as the desktop HistoryView tab; no watcher push).
        "tail" => {
            let path = params.get("path").and_then(Value::as_str).ok_or("missing path")?;
            let n = params.get("n").and_then(Value::as_u64).unwrap_or(200) as usize;
            let sources = crate::agent_source::build_sources();
            let source = crate::agent_source::find_source_for_path(&sources, path)
                .ok_or_else(|| format!("no agent source for path: {path}"))?;
            source.get_messages_tail(path, n).map(Value::Array)
        }
        // Byte-offset incremental tail (mirrors the `/tail` endpoint): the
        // client polls with its last `newOffset` and receives only the lines
        // appended since. Omitting `offset` locates the current end without
        // reading the body — the cheap "start following from here" call.
        "tail_delta" => {
            use std::io::{Read as _, Seek as _};
            let path = params.get("path").and_then(Value::as_str).ok_or("missing path")?;
            let sources = crate::agent_source::build_sources();
            let resolved = crate::agent_source::find_source_for_path(&sources, path)
                .and_then(|s| s.resolve_file_path(path))
                .ok_or_else(|| format!("cannot resolve path: {path}"))?;
            let size = std::fs::metadata(&resolved).map_err(|e| e.to_string())?.len();
            let offset = params.get("offset").and_then(Value::as_u64);
            let Some(offset) = offset else {
                return Ok(json!({ "lines": [], "newOffset": size }));
            };
            // Truncated/rotated file (offset past EOF): resync without lines.
            if offset >= size {
                return Ok(json!({ "lines": [], "newOffset": size }));
            }
            let mut file = std::fs::File::open(&resolved).map_err(|e| e.to_string())?;
            file.seek(std::io::SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
            let mut buf = String::new();
            file.read_to_string(&mut buf).map_err(|e| e.to_string())?;
            let lines: Vec<Value> = buf
                .lines()
                .filter(|l| !l.trim().is_empty())
                .filter_map(|l| serde_json::from_str(l).ok())
                .collect();
            Ok(json!({ "lines": lines, "newOffset": size }))
        }
        // Serializes to `null` when there's no live sidecar for this session.
        "live_thinking" => {
            let session_id = params
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or("missing sessionId")?;
            serde_json::to_value(crate::live_thinking::read_live_thinking(session_id))
                .map_err(|e| e.to_string())
        }
        // `null` when the session isn't on any relay chain.
        "handoff_chain" => {
            let session_id = params
                .get("sessionId")
                .and_then(Value::as_str)
                .ok_or("missing sessionId")?;
            serde_json::to_value(crate::handoff::chain_containing(session_id))
                .map_err(|e| e.to_string())
        }
        "workflow_trees" => {
            let path = params.get("path").and_then(Value::as_str).ok_or("missing path")?;
            let trees =
                crate::workflow::discover_workflow_trees(std::path::Path::new(path));
            serde_json::to_value(trees).map_err(|e| e.to_string())
        }
        "token_breakdown" => {
            let path = params.get("path").and_then(Value::as_str).ok_or("missing path")?;
            let project_root = params.get("projectRoot").and_then(Value::as_str);
            let breakdown = crate::token_analysis::aggregate_task(
                std::path::Path::new(path),
                project_root.map(std::path::Path::new),
            )?;
            serde_json::to_value(breakdown).map_err(|e| e.to_string())
        }
        // Main-session invocations plus subagent sidecars, sorted by timestamp
        // (mirrors `LocalBackend::get_skill_history` / `/skill_history`).
        "skill_history" => {
            let path = params.get("path").and_then(Value::as_str).ok_or("missing path")?;
            let sources = crate::agent_source::build_sources();
            let source = crate::agent_source::find_source_for_path(&sources, path)
                .ok_or_else(|| format!("no agent source for path: {path}"))?;
            use crate::skill_history;
            let main_msgs = source.get_messages(path)?;
            let mut out = skill_history::extract_from_messages(&main_msgs, false);
            for sub in skill_history::subagent_jsonl_paths(std::path::Path::new(path)) {
                let sub_str = sub.to_string_lossy().to_string();
                // Best-effort: a broken subagent file shouldn't lose the rest.
                let Ok(msgs) = source.get_messages(&sub_str) else { continue };
                out.extend(skill_history::extract_from_messages(&msgs, true));
            }
            skill_history::sort_by_timestamp(&mut out);
            serde_json::to_value(out).map_err(|e| e.to_string())
        }
        // LLM risk analysis for a guard card (mirrors `/guard/analyze` and the
        // desktop's `analyze_guard_command`). Synchronous up to 30s — fine, we
        // run inside `spawn_blocking` so other frames keep flowing.
        "guard_analyze" => {
            let command =
                params.get("command").and_then(Value::as_str).ok_or("missing command")?;
            let context = params.get("context").and_then(Value::as_str).unwrap_or("");
            let lang = params.get("lang").and_then(Value::as_str).unwrap_or("zh");
            let risk_tags = crate::audit::classify_bash_command_pub(command)
                .map(|(_, tags)| tags)
                .unwrap_or_default();
            let prompt = crate::guard::build_analysis_prompt(command, &risk_tags, context, lang);
            let cfg = crate::llm_provider::shared_config();
            if cfg.provider == "none" {
                return Err("LLM provider is disabled".into());
            }
            let provider = crate::llm_provider::resolve_provider(&cfg.provider)
                .ok_or("LLM provider not available")?;
            if !provider.is_available() {
                return Err(format!("{} CLI not found", provider.display_name()));
            }
            let analysis = crate::llm_usage::complete_accounted(
                provider.as_ref(),
                &prompt,
                &cfg.fast_model,
                Duration::from_secs(30),
                crate::llm_usage::SCENARIO_GUARD_COMMAND,
            )
            .ok_or("LLM analysis timed out or failed")?;
            Ok(json!({ "analysis": analysis }))
        }
        // Full-text search over the local session index — the mobile task list
        // wires this to the same FTS the desktop launchpad uses (search_sessions
        // Tauri command → SearchIndex::search). Returns SearchHit rows
        // (sessionId / jsonlPath / snippet with <mark> markers / rank); the
        // client resolves them against its own sessions snapshot by jsonlPath.
        "session_search" => {
            let query = params.get("query").and_then(Value::as_str).unwrap_or("");
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50) as usize;
            if query.trim().len() < 2 {
                return Ok(Value::Array(Vec::new()));
            }
            let index = crate::search_index::SearchIndex::open()?;
            let hits = index.search(query, limit)?;
            serde_json::to_value(hits).map_err(|e| e.to_string())
        }
        // Wiki knowledge base: every published doc, newest-updated first
        // (mirrors the desktop's `list_wiki_docs` Tauri command).
        "wiki_list" => {
            serde_json::to_value(crate::wiki::list_docs()).map_err(|e| e.to_string())
        }
        // One file — entry or asset — from a wiki doc version, base64-framed so
        // the mobile client can decode markdown/html text or blob-serve assets
        // (imgs/css/js) referenced by an htmlDir doc. `version` defaults to the
        // current one. The 16 MiB per-file cap keeps a single frame bounded even
        // though a whole doc can reach 100 MiB.
        "wiki_file" => {
            use base64::Engine as _;
            let slug = params.get("slug").and_then(Value::as_str).ok_or("missing slug")?;
            let version =
                params.get("version").and_then(Value::as_str).unwrap_or("current");
            let relpath =
                params.get("relpath").and_then(Value::as_str).ok_or("missing relpath")?;
            let file = crate::wiki::get_file(slug, version, relpath)?;
            const MAX_WIKI_FILE_BYTES: usize = 16 * 1024 * 1024;
            if file.bytes.len() > MAX_WIKI_FILE_BYTES {
                return Err(format!("wiki file too large: {} bytes", file.bytes.len()));
            }
            Ok(json!({
                "mime": file.mime,
                "base64": base64::engine::general_purpose::STANDARD.encode(&file.bytes),
            }))
        }
        // ── Write methods ────────────────────────────────────────────────
        // All of these run inside `spawn_blocking` (see ws_connect_once), so
        // process spawns / kills / file writes never block the ws runtime.
        "spawn_session" => {
            let req: crate::session_launch::SpawnSessionRequest =
                serde_json::from_value(params.clone())
                    .map_err(|e| format!("bad spawn_session params: {e}"))?;
            // Thread the phone-provided session id through so a dropped reply
            // frame is still recoverable: the phone confirms success by finding
            // this exact id in a later `sessions` snapshot (relay delivery is
            // best-effort — see registry::forward).
            let resp = crate::session_launch::spawn_new_session_with_id(
                &req.workspace_path,
                &req.prompt,
                req.model.as_deref(),
                req.effort.as_deref(),
                req.permission_mode.as_deref(),
                req.session_id.as_deref(),
            )?;
            serde_json::to_value(resp).map_err(|e| e.to_string())
        }
        "resume_session" => {
            let req: crate::auto_resume::ResumeSessionRequest =
                serde_json::from_value(params.clone())
                    .map_err(|e| format!("bad resume_session params: {e}"))?;
            crate::auto_resume::spawn_resume_prompt(
                &req.session_id,
                &req.workspace_path,
                req.prompt.as_deref().unwrap_or("continue"),
                req.model.as_deref(),
                req.effort.as_deref(),
                req.permission_mode.as_deref(),
            )?;
            Ok(json!({ "ok": true }))
        }
        // pid 0 would signal the desktop's own process group — reject before
        // it ever reaches kill() (same guard as the `/interrupt` endpoint).
        "interrupt" => {
            let pid = params.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
            if pid == 0 {
                return Err("missing or invalid pid".into());
            }
            crate::session::interrupt_pid_impl(pid)?;
            Ok(json!({ "ok": true }))
        }
        "stop" => {
            let pid = params.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
            if pid == 0 {
                return Err("missing or invalid pid".into());
            }
            let force = params.get("force").and_then(Value::as_bool).unwrap_or(false);
            #[cfg(unix)]
            {
                // Probe first so a stale pid errors; then take the whole tree,
                // or the agent's tool children outlive it.
                if unsafe { libc::kill(pid as libc::pid_t, 0) } != 0 {
                    return Err(std::io::Error::last_os_error().to_string());
                }
                crate::session::kill_pid_tree(pid, force)?;
                Ok(json!({ "ok": true }))
            }
            #[cfg(not(unix))]
            {
                let _ = force;
                Err("stop is not supported on this platform".into())
            }
        }
        "stop_workspace" => {
            let path = params
                .get("workspacePath")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .ok_or("missing workspacePath")?;
            crate::session::kill_workspace_impl(path)?;
            Ok(json!({ "ok": true }))
        }
        "session_mark" => {
            let req: crate::session_mark::SetSessionMarkRequest =
                serde_json::from_value(params.clone())
                    .map_err(|e| format!("bad session_mark params: {e}"))?;
            crate::session_mark::set_mark(&req.session_id, &req.workspace_path, req.mark)?;
            Ok(json!({ "ok": true }))
        }
        // Mobile-side attachment bytes → the desktop's content-addressed
        // user-attachments store. The returned absolute path is what the
        // client splices into an answer (`@<path>`) or a new-session prompt —
        // shared by the decision panel and the composer flows.
        "upload_attachment" => {
            use base64::Engine as _;
            let name = params.get("name").and_then(Value::as_str).unwrap_or("attachment.bin");
            let b64 = params.get("base64").and_then(Value::as_str).ok_or("missing base64")?;
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map_err(|e| format!("invalid base64: {e}"))?;
            if bytes.len() as u64 > MAX_UPLOAD_BYTES {
                return Err(format!(
                    "attachment too large: {} bytes (max {MAX_UPLOAD_BYTES})",
                    bytes.len()
                ));
            }
            let stored = crate::user_attachments::ingest_bytes(&bytes, name)?;
            Ok(json!({ "path": stored.to_string_lossy() }))
        }
        // Given a list of previously-uploaded attachment paths, return the
        // subset still present in the user-attachments store. The mobile
        // composer persists attachment chips across reloads and uses this to
        // drop ones whose backing file has been cleared, so a restored draft
        // never carries a `Context files:` path that no longer resolves.
        "attachments_exist" => {
            let paths = params.get("paths").and_then(Value::as_array).ok_or("missing paths")?;
            let existing: Vec<String> = paths
                .iter()
                .filter_map(Value::as_str)
                .filter(|p| {
                    crate::user_attachments::exists_in_store(std::path::Path::new(p))
                })
                .map(str::to_string)
                .collect();
            Ok(json!({ "existing": existing }))
        }
        "session_read" => {
            let req: crate::session_read::MarkSessionsReadRequest =
                serde_json::from_value(params.clone())
                    .map_err(|e| format!("bad session_read params: {e}"))?;
            crate::session_read::mark_read(&req.items)?;
            Ok(json!({ "ok": true }))
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
                        // Fresh socket — drop any stale device registry; clients
                        // re-announce themselves on their next heartbeat.
                        clear_clients();
                        SESSIONS_LAST_HASH.store(0, Ordering::SeqCst);
                        crate::log_debug("[mobile-relay] connected");
                    }
                    Some("presence") => {
                        if let Some(n) = frame.get("clients").and_then(Value::as_u64) {
                            let prev = CLIENTS.swap(n as usize, Ordering::SeqCst);
                            // A client just joined: drop the dedup hash so the
                            // next scan tick pushes the current state to it
                            // even when nothing changed for existing clients.
                            if n as usize > prev {
                                SESSIONS_LAST_HASH.store(0, Ordering::SeqCst);
                            }
                            // Everyone's gone: forget the device list immediately
                            // rather than waiting out the stale timeout.
                            if n == 0 {
                                clear_clients();
                            }
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
    fn answer_a2ui_render_writes_response_file() {
        with_temp_home(|| {
            let dir = fleet_subdir("fleet-render-a2ui");
            handle_client_payload(&json!({
                "event": "answer", "kind": "a2ui-render", "id": "a1",
                "cancelled": false, "actionName": "submit",
                "actionContext": { "score": "7" }
            }));
            let v = read_json(&dir.join("a1.response.json"));
            assert_eq!(v["cancelled"], false);
            assert_eq!(v["actionName"], "submit");
            assert_eq!(v["actionContext"]["score"], "7");

            handle_client_payload(&json!({
                "event": "answer", "kind": "a2ui-render", "id": "a2",
                "cancelled": true
            }));
            let v = read_json(&dir.join("a2.response.json"));
            assert_eq!(v["cancelled"], true);
            assert!(v.get("actionName").is_none());
        });
    }

    #[test]
    fn pending_snapshot_includes_a2ui_render() {
        with_temp_home(|| {
            let dir = fleet_subdir("fleet-render-a2ui");
            fs::write(
                dir.join("a9.json"),
                serde_json::to_string(&json!({
                    "id": "a9", "sessionId": "s1", "workspaceName": "demo",
                    "timestamp": "2026-07-12T10:00:00Z",
                    "messageTree": { "surfaceUpdate": { "surfaceId": "x" } }
                }))
                .unwrap(),
            )
            .unwrap();
            let reply = handle_client_payload(&json!({
                "event": "req", "req_id": "r2", "method": "pending_snapshot"
            }))
            .expect("pending_snapshot must produce a reply");
            let list = reply["data"]["a2uiRender"].as_array().expect("a2uiRender list");
            assert_eq!(list.len(), 1);
            assert_eq!(list[0]["id"], "a9");
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

    /// Issue a `req` payload and unwrap the ok reply's `data`.
    fn request_ok(method: &str, params: Value) -> Value {
        let reply = handle_client_payload(&json!({
            "event": "req", "req_id": "r", "method": method, "params": params
        }))
        .unwrap_or_else(|| panic!("{method} must produce a reply"));
        assert_eq!(reply["ok"], true, "{method} failed: {}", reply["error"]);
        reply["data"].clone()
    }

    /// Write a jsonl transcript under the temp FLEET_HOME and return its path.
    fn write_jsonl(name: &str, lines: &[Value]) -> String {
        let dir = crate::session::real_home_dir().unwrap().join("transcripts");
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join(name);
        let body: String = lines.iter().map(|l| format!("{l}\n")).collect();
        fs::write(&p, body).unwrap();
        p.to_string_lossy().to_string()
    }

    #[test]
    fn request_tail_returns_last_n_messages() {
        with_temp_home(|| {
            let path = write_jsonl(
                "t1.jsonl",
                &[
                    json!({"type":"user","message":{"content":"one"}}),
                    json!({"type":"assistant","message":{"content":"two"}}),
                    json!({"type":"user","message":{"content":"three"}}),
                ],
            );
            let data = request_ok("tail", json!({"path": path, "n": 2}));
            let msgs = data.as_array().expect("array");
            assert_eq!(msgs.len(), 2);
            assert_eq!(msgs[0]["message"]["content"], "two");
            assert_eq!(msgs[1]["message"]["content"], "three");

            // n larger than the file returns everything.
            let data = request_ok("tail", json!({"path": path, "n": 99}));
            assert_eq!(data.as_array().unwrap().len(), 3);
        });
    }

    /// base64-decode a `wiki_file` reply's payload back to bytes.
    fn decode_b64(data: &Value) -> Vec<u8> {
        use base64::Engine as _;
        base64::engine::general_purpose::STANDARD
            .decode(data["base64"].as_str().expect("base64 string"))
            .expect("valid base64")
    }

    #[test]
    fn request_wiki_list_and_markdown_file() {
        with_temp_home(|| {
            // Publish a markdown doc into the temp-home wiki root.
            let src = crate::session::real_home_dir().unwrap().join("wiki-src");
            fs::create_dir_all(&src).unwrap();
            let file = src.join("note.md");
            fs::write(&file, "# 标题\n\n正文内容 body").unwrap();
            let doc =
                crate::wiki::publish(&file, None, None, std::path::Path::new("/ws/demo")).unwrap();

            // wiki_list surfaces it.
            let data = request_ok("wiki_list", Value::Null);
            let docs = data.as_array().expect("array");
            assert!(docs.iter().any(|d| d["slug"] == doc.slug), "listed docs miss the publish");

            // wiki_file returns the entry, base64-framed; decode back to text.
            let data =
                request_ok("wiki_file", json!({"slug": doc.slug, "relpath": doc.entry}));
            let text = String::from_utf8(decode_b64(&data)).unwrap();
            assert!(text.contains("正文内容"), "entry text not round-tripped: {text}");
            assert!(data["mime"].as_str().unwrap().contains("markdown"));
        });
    }

    #[test]
    fn request_wiki_file_serves_asset_and_blocks_traversal() {
        with_temp_home(|| {
            // htmlDir doc: index.html plus a css asset in a subdir.
            let root = crate::session::real_home_dir().unwrap().join("wiki-site");
            fs::create_dir_all(root.join("assets")).unwrap();
            fs::write(root.join("index.html"), "<link href=\"assets/app.css\">").unwrap();
            fs::write(root.join("assets/app.css"), "body{color:red}").unwrap();
            let doc =
                crate::wiki::publish(&root, None, None, std::path::Path::new("/ws/demo")).unwrap();
            assert_eq!(doc.kind, "htmlDir");

            // A referenced asset is served with its own mime.
            let data = request_ok(
                "wiki_file",
                json!({"slug": doc.slug, "relpath": "assets/app.css"}),
            );
            assert_eq!(String::from_utf8(decode_b64(&data)).unwrap(), "body{color:red}");
            assert!(data["mime"].as_str().unwrap().contains("css"));

            // Path traversal is rejected (get_file's canonicalize guard).
            let reply = handle_client_payload(&json!({
                "event": "req", "req_id": "r", "method": "wiki_file",
                "params": {"slug": doc.slug, "relpath": "../../../etc/passwd"}
            }))
            .expect("reply");
            assert_eq!(reply["ok"], false, "traversal must be rejected");
        });
    }

    #[test]
    fn request_live_thinking_reads_fresh_sidecar() {
        with_temp_home(|| {
            let dir = fleet_subdir("live-thinking");
            fs::write(
                dir.join("pid-1.jsonl"),
                concat!(
                    r#"{"type":"system","session_id":"abc"}"#, "\n",
                    r#"{"type":"stream_event","event":{"type":"message_start"},"session_id":"abc"}"#, "\n",
                    r#"{"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}},"session_id":"abc"}"#, "\n",
                    r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"深思熟虑"}},"session_id":"abc"}"#, "\n",
                ),
            )
            .unwrap();
            let data = request_ok("live_thinking", json!({"sessionId": "abc"}));
            assert_eq!(data["sessionId"], "abc");
            assert_eq!(data["thinking"], "深思熟虑");

            // Unknown session → null, not an error.
            let data = request_ok("live_thinking", json!({"sessionId": "nope"}));
            assert!(data.is_null());
        });
    }

    #[test]
    fn request_handoff_chain_finds_chain() {
        with_temp_home(|| {
            let dir = crate::session::real_home_dir()
                .unwrap()
                .join(".fleet")
                .join("handoffs")
                .join("chain");
            fs::create_dir_all(&dir).unwrap();
            fs::write(
                dir.join("c1.json"),
                json!({
                    "chainId": "c1",
                    "workspacePath": "/ws",
                    "links": [{
                        "fromSessionId": "s1", "toSessionId": "s2",
                        "note": "交接", "handedAt": 1
                    }]
                })
                .to_string(),
            )
            .unwrap();
            let data = request_ok("handoff_chain", json!({"sessionId": "s2"}));
            assert_eq!(data["chainId"], "c1");
            assert_eq!(data["links"][0]["fromSessionId"], "s1");

            let data = request_ok("handoff_chain", json!({"sessionId": "unknown"}));
            assert!(data.is_null());
        });
    }

    #[test]
    fn request_workflow_trees_reads_journal() {
        with_temp_home(|| {
            let path = write_jsonl("wf-main.jsonl", &[json!({"type":"user"})]);
            // Session dir = jsonl path minus extension; workflows live under
            // <session-dir>/subagents/workflows/wf_*/journal.jsonl.
            let wf_dir = crate::session::real_home_dir()
                .unwrap()
                .join("transcripts")
                .join("wf-main")
                .join("subagents")
                .join("workflows")
                .join("wf_r1");
            fs::create_dir_all(&wf_dir).unwrap();
            fs::write(
                wf_dir.join("journal.jsonl"),
                r#"{"type":"started","key":"v2:aaa","agentId":"agentA"}"#,
            )
            .unwrap();
            let data = request_ok("workflow_trees", json!({"path": path}));
            let trees = data.as_array().expect("array");
            assert_eq!(trees.len(), 1);
            assert_eq!(trees[0]["runId"], "wf_r1");
            assert_eq!(trees[0]["agents"].as_array().unwrap().len(), 1);

            // A session with no workflow dir yields an empty list.
            let bare = write_jsonl("bare.jsonl", &[json!({"type":"user"})]);
            let data = request_ok("workflow_trees", json!({"path": bare}));
            assert_eq!(data.as_array().unwrap().len(), 0);
        });
    }

    #[test]
    fn request_token_breakdown_aggregates_usage() {
        with_temp_home(|| {
            let path = write_jsonl(
                "tok.jsonl",
                &[
                    json!({"type":"user","message":{"role":"user","content":[{"type":"text","text":"hi"}]}}),
                    json!({"type":"assistant","message":{
                        "model":"claude-opus-4-7",
                        "stop_reason":"end_turn",
                        "content":[{"type":"text","text":"hello"}],
                        "usage":{"input_tokens":1,"output_tokens":10,
                                 "cache_creation_input_tokens":15000,"cache_read_input_tokens":0}
                    }}),
                ],
            );
            let data = request_ok("token_breakdown", json!({"path": path}));
            assert_eq!(data["totalsUsage"]["outputTokens"], 10);
            assert!(data["main"].is_object());
            assert_eq!(data["subagents"].as_array().unwrap().len(), 0);
        });
    }

    #[test]
    fn request_skill_history_extracts_invocations() {
        with_temp_home(|| {
            let path = write_jsonl(
                "sk.jsonl",
                &[json!({
                    "type": "assistant",
                    "timestamp": "2026-07-12T10:00:00Z",
                    "message": {"content": [{
                        "type": "tool_use", "name": "Skill",
                        "input": {"skill": "code-review", "args": "--fix"}
                    }]}
                })],
            );
            let data = request_ok("skill_history", json!({"path": path}));
            let items = data.as_array().expect("array");
            assert_eq!(items.len(), 1);
            assert_eq!(items[0]["skill"], "code-review");
            assert_eq!(items[0]["args"], "--fix");
            assert_eq!(items[0]["isSubagent"], false);
        });
    }

    /// Issue a `req` payload and return the full reply (ok or error).
    fn request_raw(method: &str, params: Value) -> Value {
        handle_client_payload(&json!({
            "event": "req", "req_id": "r", "method": method, "params": params
        }))
        .unwrap_or_else(|| panic!("{method} must produce a reply"))
    }

    #[test]
    fn request_session_mark_sets_and_clears() {
        with_temp_home(|| {
            let data = request_ok(
                "session_mark",
                json!({"sessionId": "s1", "workspacePath": "/ws", "mark": "pending"}),
            );
            assert_eq!(data["ok"], true);
            assert_eq!(crate::session_mark::read("s1"), Some(crate::session_mark::SessionMark::Pending));

            request_ok(
                "session_mark",
                json!({"sessionId": "s1", "workspacePath": "/ws", "mark": "done"}),
            );
            assert_eq!(crate::session_mark::read("s1"), Some(crate::session_mark::SessionMark::Done));

            // Omitting `mark` clears the record.
            request_ok("session_mark", json!({"sessionId": "s1", "workspacePath": "/ws"}));
            assert_eq!(crate::session_mark::read("s1"), None);
        });
    }

    #[test]
    fn request_session_search_finds_indexed_content() {
        with_temp_home(|| {
            // Index a transcript into the on-disk FTS db under the temp home
            // (SearchIndex::open resolves to <temp>/.fleet/fleet-search.db).
            let path = write_jsonl(
                "searchable.jsonl",
                &[
                    json!({"type": "ai-title", "aiTitle": "peculiar zebra migration"}),
                    json!({"type": "user", "message": {"content": "hello world"}}),
                ],
            );
            let index = crate::search_index::SearchIndex::open().unwrap();
            index.index_session(&path, "sess-search").unwrap();

            // A hit is returned as a SearchHit row keyed by jsonlPath.
            let data = request_ok(
                "session_search",
                json!({"query": "peculiar zebra", "limit": 10}),
            );
            let hits = data.as_array().expect("hits array");
            assert_eq!(hits.len(), 1, "distinctive phrase should match one session");
            assert_eq!(hits[0]["sessionId"], "sess-search");
            assert_eq!(hits[0]["jsonlPath"], path);

            // Queries shorter than 2 chars short-circuit to an empty list
            // (the client-side substring filter handles those).
            let short = request_ok("session_search", json!({"query": "z"}));
            assert_eq!(short.as_array().expect("array").len(), 0);
        });
    }

    #[test]
    fn request_session_read_stamps_records() {
        with_temp_home(|| {
            let data = request_ok(
                "session_read",
                json!({"items": [
                    {"sessionId": "s1", "workspacePath": "/ws"},
                    {"sessionId": "s2", "workspacePath": "/ws"},
                ]}),
            );
            assert_eq!(data["ok"], true);
            let dir = crate::session::real_home_dir()
                .unwrap()
                .join(".fleet")
                .join("session-read");
            assert!(dir.join("s1.json").is_file());
            assert!(dir.join("s2.json").is_file());
        });
    }

    #[test]
    fn request_interrupt_and_stop_reject_bad_pid() {
        with_temp_home(|| {
            // pid 0 would signal the caller's own process group — must be
            // rejected in the relay layer before reaching kill().
            for method in ["interrupt", "stop"] {
                let reply = request_raw(method, json!({"pid": 0}));
                assert_eq!(reply["ok"], false, "{method} must reject pid 0");
                assert!(
                    reply["error"].as_str().unwrap().contains("pid"),
                    "{method} error must mention pid: {}",
                    reply["error"]
                );
                let reply = request_raw(method, json!({}));
                assert_eq!(reply["ok"], false, "{method} must reject missing pid");
            }
        });
    }

    #[cfg(unix)]
    #[test]
    fn request_stop_kills_spawned_process() {
        with_temp_home(|| {
            let mut child = std::process::Command::new("sleep")
                .arg("30")
                .spawn()
                .expect("spawn sleep");
            let pid = child.id();
            let data = request_ok("stop", json!({"pid": pid, "force": true}));
            assert_eq!(data["ok"], true);
            // The tree kill is synchronous, but give the OS a beat to deliver.
            let mut waited = 0u64;
            loop {
                match child.try_wait().expect("try_wait") {
                    Some(_) => break,
                    None if waited >= 5000 => panic!("sleep child survived stop"),
                    None => {
                        std::thread::sleep(Duration::from_millis(50));
                        waited += 50;
                    }
                }
            }
        });
    }

    #[test]
    fn request_stop_workspace_requires_path() {
        with_temp_home(|| {
            let reply = request_raw("stop_workspace", json!({}));
            assert_eq!(reply["ok"], false);
            assert!(
                reply["error"].as_str().unwrap().contains("workspacePath"),
                "error must mention workspacePath: {}",
                reply["error"]
            );
        });
    }

    #[test]
    fn request_spawn_session_validates_before_spawning() {
        with_temp_home(|| {
            // Empty prompt fails validation before any CLI/filesystem access.
            let reply = request_raw(
                "spawn_session",
                json!({"workspacePath": "/ws", "prompt": "   "}),
            );
            assert_eq!(reply["ok"], false);
            assert!(reply["error"].as_str().unwrap().contains("prompt"));

            let reply = request_raw("spawn_session", json!({"prompt": "hi"}));
            assert_eq!(reply["ok"], false, "missing workspacePath must fail");

            let reply = request_raw(
                "spawn_session",
                json!({"workspacePath": "/ws", "prompt": "hi", "permissionMode": "yolo"}),
            );
            assert_eq!(reply["ok"], false);
            assert!(reply["error"].as_str().unwrap().contains("permission mode"));
        });
    }

    #[test]
    fn request_resume_session_validates_before_spawning() {
        with_temp_home(|| {
            let reply = request_raw("resume_session", json!({"workspacePath": "/ws"}));
            assert_eq!(reply["ok"], false, "missing sessionId must fail");

            let reply = request_raw(
                "resume_session",
                json!({"sessionId": "s1", "workspacePath": "/ws", "permissionMode": "yolo"}),
            );
            assert_eq!(reply["ok"], false);
            assert!(reply["error"].as_str().unwrap().contains("permission mode"));
        });
    }

    #[test]
    fn request_upload_attachment_ingests_bytes() {
        use base64::Engine as _;
        with_temp_home(|| {
            let bytes = b"fake image bytes \xe6\x88\xaa\xe5\x9b\xbe";
            let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
            let data = request_ok(
                "upload_attachment",
                json!({"name": "screenshot.png", "base64": b64}),
            );
            let path = data["path"].as_str().expect("path string");
            let stored = std::path::Path::new(path);
            assert!(stored.is_file(), "attachment must land on disk");
            assert_eq!(fs::read(stored).unwrap(), bytes);
            let store_root = crate::user_attachments::user_attachments_dir().unwrap();
            assert!(
                stored.starts_with(&store_root),
                "must live under the user-attachments store: {path}"
            );
            assert!(path.ends_with("screenshot.png"));

            // Content-addressed: identical bytes → identical path.
            let again = request_ok(
                "upload_attachment",
                json!({"name": "screenshot.png", "base64": b64}),
            );
            assert_eq!(again["path"], data["path"]);
        });
    }

    #[test]
    fn request_attachments_exist_returns_only_present_paths() {
        use base64::Engine as _;
        with_temp_home(|| {
            let b64 = base64::engine::general_purpose::STANDARD.encode(b"real bytes");
            let uploaded = request_ok("upload_attachment", json!({"name": "a.png", "base64": b64}));
            let real = uploaded["path"].as_str().unwrap().to_string();
            let store_root = crate::user_attachments::user_attachments_dir().unwrap();
            let ghost = store_root.join("deadbeef").join("gone.png");

            let data = request_ok(
                "attachments_exist",
                json!({"paths": [real, ghost.to_string_lossy(), "/etc/passwd"]}),
            );
            let existing: Vec<String> = data["existing"]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_str().unwrap().to_string())
                .collect();
            // Only the genuinely-present in-store file survives; the cleared one
            // and the out-of-store probe are dropped.
            assert_eq!(existing.len(), 1);
            assert!(existing[0].ends_with("a.png"));
        });
    }

    #[test]
    fn request_attachments_exist_requires_paths_array() {
        with_temp_home(|| {
            let reply = request_raw("attachments_exist", json!({}));
            assert_eq!(reply["ok"], false, "missing paths must fail");
        });
    }

    #[test]
    fn request_upload_attachment_sanitizes_hostile_name() {
        use base64::Engine as _;
        with_temp_home(|| {
            let b64 = base64::engine::general_purpose::STANDARD.encode(b"x");
            let data = request_ok(
                "upload_attachment",
                json!({"name": "../../evil.sh", "base64": b64}),
            );
            let path = std::path::PathBuf::from(data["path"].as_str().unwrap());
            let store_root = crate::user_attachments::user_attachments_dir().unwrap();
            assert!(
                path.canonicalize().unwrap().starts_with(store_root.canonicalize().unwrap()),
                "sanitized path must stay inside the store: {}",
                path.display()
            );
        });
    }

    #[test]
    fn request_upload_attachment_rejects_bad_input() {
        with_temp_home(|| {
            let reply = request_raw("upload_attachment", json!({"name": "a.png"}));
            assert_eq!(reply["ok"], false, "missing base64 must fail");

            let reply =
                request_raw("upload_attachment", json!({"name": "a.png", "base64": "!!!"}));
            assert_eq!(reply["ok"], false, "invalid base64 must fail");
            assert!(reply["error"].as_str().unwrap().contains("base64"));
        });
    }

    #[test]
    fn request_upload_attachment_rejects_oversize() {
        use base64::Engine as _;
        with_temp_home(|| {
            let bytes = vec![0u8; (MAX_UPLOAD_BYTES + 1) as usize];
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            let reply = request_raw("upload_attachment", json!({"name": "big.bin", "base64": b64}));
            assert_eq!(reply["ok"], false);
            assert!(reply["error"].as_str().unwrap().contains("too large"));
        });
    }

    #[test]
    fn answer_guard_always_allow_persists_rule() {
        with_temp_home(|| {
            let dir = fleet_subdir("guard");
            handle_client_payload(&json!({
                "event": "answer", "kind": "guard", "id": "g10",
                "allow": true,
                "alwaysAllow": { "prefix": "git push", "sourceTag": "eval-exec" }
            }));
            let v = read_json(&dir.join("g10.response.json"));
            assert_eq!(v["decision"], "allow");
            let rules = crate::audit::list_guard_allow_rules();
            assert!(
                rules.iter().any(|r| r.prefix == "git push"),
                "always-allow rule must be persisted: {rules:?}"
            );

            // Block + alwaysAllow is nonsensical — must NOT persist a rule.
            handle_client_payload(&json!({
                "event": "answer", "kind": "guard", "id": "g11",
                "allow": false,
                "alwaysAllow": { "prefix": "rm -rf" }
            }));
            let v = read_json(&dir.join("g11.response.json"));
            assert_eq!(v["decision"], "block");
            let rules = crate::audit::list_guard_allow_rules();
            assert!(
                !rules.iter().any(|r| r.prefix == "rm -rf"),
                "block must not persist an allow rule: {rules:?}"
            );
        });
    }

    #[test]
    fn request_guard_analyze_respects_disabled_provider() {
        with_temp_home(|| {
            let prev = crate::llm_provider::shared_config();
            crate::llm_provider::set_shared_config(crate::llm_provider::LlmConfig {
                provider: "none".into(),
                ..Default::default()
            });
            let reply = request_raw(
                "guard_analyze",
                json!({"command": "rm -rf /tmp/x", "context": "", "lang": "zh"}),
            );
            crate::llm_provider::set_shared_config(prev);
            assert_eq!(reply["ok"], false);
            assert!(
                reply["error"].as_str().unwrap().contains("disabled"),
                "must surface the disabled provider: {}",
                reply["error"]
            );

            let reply = request_raw("guard_analyze", json!({}));
            assert_eq!(reply["ok"], false, "missing command must fail");
        });
    }

    #[test]
    fn request_tail_delta_incremental_reads() {
        with_temp_home(|| {
            let path = write_jsonl(
                "delta.jsonl",
                &[json!({"type":"user","uuid":"u1"}), json!({"type":"assistant","uuid":"a1"})],
            );

            // No offset → locate the current end without reading the body.
            let data = request_ok("tail_delta", json!({"path": path}));
            assert_eq!(data["lines"].as_array().unwrap().len(), 0);
            let end = data["newOffset"].as_u64().expect("newOffset");
            assert!(end > 0);

            // offset 0 → everything, newOffset = file size.
            let data = request_ok("tail_delta", json!({"path": path, "offset": 0}));
            assert_eq!(data["lines"].as_array().unwrap().len(), 2);
            assert_eq!(data["newOffset"].as_u64().unwrap(), end);

            // Append one line; polling from `end` returns only the new line.
            {
                use std::io::Write as _;
                let mut f = fs::OpenOptions::new().append(true).open(&path).unwrap();
                writeln!(f, r#"{{"type":"assistant","uuid":"a2"}}"#).unwrap();
            }
            let data = request_ok("tail_delta", json!({"path": path, "offset": end}));
            let lines = data["lines"].as_array().unwrap();
            assert_eq!(lines.len(), 1);
            assert_eq!(lines[0]["uuid"], "a2");
            let end2 = data["newOffset"].as_u64().unwrap();
            assert!(end2 > end);

            // Caught up: no new bytes → empty, offset unchanged.
            let data = request_ok("tail_delta", json!({"path": path, "offset": end2}));
            assert_eq!(data["lines"].as_array().unwrap().len(), 0);
            assert_eq!(data["newOffset"].as_u64().unwrap(), end2);

            // Offset beyond the file (truncated/rotated) → resync to real size.
            let data = request_ok("tail_delta", json!({"path": path, "offset": end2 + 999}));
            assert_eq!(data["lines"].as_array().unwrap().len(), 0);
            assert_eq!(data["newOffset"].as_u64().unwrap(), end2);
        });
    }

    #[test]
    fn slim_snapshot_filters_and_whitelists() {
        let sessions = json!([
            {
                "id": "old", "isSubagent": false, "lastActivityMs": 100,
                "workspaceName": "w", "tokenSpeed": 42.0, "rateLimit": {"until": 1},
                "lastMessagePreview": "短预览", "userMark": "done", "aiTitle": null
            },
            {
                "id": "sub", "isSubagent": true, "lastActivityMs": 300,
                "workspaceName": "w"
            },
            {
                "id": "new", "isSubagent": false, "lastActivityMs": 200,
                "workspaceName": "w",
                "lastMessagePreview": "长".repeat(500)
            },
        ]);
        let slim = slim_sessions_snapshot(&sessions);
        let list = slim.as_array().expect("array");
        // Subagent dropped; remainder sorted most-recent first.
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["id"], "new");
        assert_eq!(list[1]["id"], "old");
        // Desktop-only fields are gone, whitelisted ones survive, nulls dropped.
        assert!(list[1].get("tokenSpeed").is_none());
        assert!(list[1].get("rateLimit").is_none());
        assert!(list[1].get("aiTitle").is_none());
        assert_eq!(list[1]["userMark"], "done");
        assert_eq!(list[1]["lastMessagePreview"], "短预览");
        // Long previews are truncated with an ellipsis.
        let preview = list[0]["lastMessagePreview"].as_str().unwrap();
        assert_eq!(preview.chars().count(), SNAPSHOT_PREVIEW_CHARS + 1);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn slim_snapshot_caps_session_count() {
        let many: Vec<Value> = (0..300)
            .map(|i| json!({"id": format!("s{i}"), "isSubagent": false, "lastActivityMs": i}))
            .collect();
        let slim = slim_sessions_snapshot(&Value::Array(many));
        let list = slim.as_array().unwrap();
        assert_eq!(list.len(), SNAPSHOT_MAX_SESSIONS);
        // Kept the MOST recent ones (highest lastActivityMs).
        assert_eq!(list[0]["id"], "s299");
        assert_eq!(list.last().unwrap()["id"], format!("s{}", 300 - SNAPSHOT_MAX_SESSIONS));
    }

    #[test]
    fn snapshot_hash_dedups_identical_content() {
        let a = slim_sessions_snapshot(&json!([{"id": "x", "lastActivityMs": 1}]));
        let b = slim_sessions_snapshot(&json!([{"id": "x", "lastActivityMs": 1}]));
        let c = slim_sessions_snapshot(&json!([{"id": "x", "lastActivityMs": 2}]));
        assert_eq!(snapshot_hash(&a), snapshot_hash(&b));
        assert_ne!(snapshot_hash(&a), snapshot_hash(&c));
        assert_ne!(snapshot_hash(&a), 0, "hash must never equal the sentinel");
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

    fn hello(client_id: &str, label: &str, push: bool) -> Value {
        json!({
            "event": "client_hello",
            "clientId": client_id,
            "label": label,
            "platform": "ios",
            "pushSubscribed": push,
        })
    }

    // One combined test: CLIENTS_REGISTRY is a process-global static, so
    // splitting into several `#[test]`s would race them against each other.
    #[test]
    fn client_registry_lifecycle() {
        clear_clients();

        // A hello registers a device; a repeat with the same id dedups and
        // refreshes metadata rather than adding a second entry.
        handle_client_payload(&hello("a", "iPhone · Safari", false));
        handle_client_payload(&hello("b", "Android · Chrome", true));
        handle_client_payload(&hello("a", "iPhone · Safari", true));
        let devices = live_devices();
        assert_eq!(devices.len(), 2, "same clientId must not duplicate");
        let a = devices.iter().find(|d| d.client_id == "a").unwrap();
        assert!(a.push_subscribed, "second hello refreshed pushSubscribed");
        assert!(a.connected_at_ms <= a.last_seen_ms, "connected_at is the earliest sighting");

        // A hello without a clientId is ignored.
        handle_client_payload(&json!({ "event": "client_hello", "label": "x" }));
        assert_eq!(live_devices().len(), 2);

        // Stale entries (older than CLIENT_STALE_MS) are pruned on read.
        {
            let mut guard = CLIENTS_REGISTRY.lock().unwrap();
            let map = guard.as_mut().unwrap();
            map.get_mut("b").unwrap().last_seen_ms = now_ms().saturating_sub(CLIENT_STALE_MS + 5_000);
        }
        let devices = live_devices();
        assert_eq!(devices.len(), 1, "stale device pruned");
        assert_eq!(devices[0].client_id, "a");

        // A bye removes immediately; clear wipes everything.
        handle_client_payload(&json!({ "event": "client_bye", "clientId": "a" }));
        assert!(live_devices().is_empty(), "bye removed the last device");
        handle_client_payload(&hello("c", "HarmonyOS · ArkWeb", false));
        clear_clients();
        assert!(live_devices().is_empty(), "clear wiped the registry");
    }

    #[test]
    fn status_devices_field_defaults_when_absent() {
        // A RemoteBackend probing an older `fleet serve` gets JSON with no
        // `devices` key — it must still deserialize (serde default = empty).
        let old: MobileRelayStatus = serde_json::from_str(
            r#"{"enabled":true,"connected":true,"clients":1,"relayUrl":"x","secretSet":true}"#,
        )
        .expect("must deserialize without a devices field");
        assert!(old.devices.is_empty());
    }
}
