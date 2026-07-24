//! Mobile relay channel — connects the desktop to a `fleet-relay` instance
//! over an *outbound* WebSocket (no inbound port, works behind NAT) so the
//! mobile web app can receive decision
//! cards / session snapshots and send answers back.
//!
//! Transport frames (relay envelope, see `fleet-relay/src/frames.rs`):
//!   out:  `auth` / `msg{payload}` / `notify{title,body,tag,url}`
//!   in:   `authed` / `presence{clients}` / `msg{payload}` / `error`
//!
//! Business frames live inside `msg.payload` and are Fleet-specific:
//!   agent → client: `decision_created{kind,request}` / `decision_resolved{kind,id}`
//!                   / `sessions{sessions}` / `ack{req_id}` / `reply{req_id,ok,data}`
//!   client → agent: `answer{kind,id,...}` / `req{req_id,method,params}`
//!                   / `client_hello{clientId,label,platform,pushSubscribed,supportsGzip,supportsBinary,supportsDelta,appCommit}` (presence)
//!                   / `client_bye{clientId}`
//!
//! Answers are written back through the same channel-agnostic
//! `write_response()` files the desktop panel uses, so the waiting
//! hook/MCP subprocess unblocks without any session-side changes.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use flate2::{write::GzEncoder, Compression};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

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
/// never reaches server logs. When `lang` is `Some("zh"|"en")` it is appended
/// to the fragment so a fresh scan brings the desktop's current UI language
/// over to the phone (`#k=<secret>&lang=<lang>`); any other value is ignored.
pub fn pairing_url(cfg: &MobileRelayConfig, lang: Option<&str>) -> String {
    let base = format!("{}/#k={}", cfg.relay_url.trim_end_matches('/'), cfg.secret);
    match lang {
        Some(l @ ("zh" | "en")) => format!("{base}&lang={l}"),
        _ => base,
    }
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

/// Render the pairing URL as an SVG QR code for the 「移动端」 view. `lang`
/// carries the desktop's current UI language into the encoded URL (see
/// [`pairing_url`]).
pub fn qr_svg(lang: Option<&str>) -> Result<String, String> {
    let cfg = load_config();
    if cfg.secret.is_empty() {
        return Err("mobile relay secret not set".into());
    }
    let code = qrcode::QrCode::new(pairing_url(&cfg, lang).as_bytes())
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
    /// Whether this client can inflate a gzipped payload (native
    /// `DecompressionStream`). Gates whether `encode_payload` gzips *before*
    /// sealing: a single client that can't inflate forces uncompressed sealing
    /// for the whole channel. Absent in a hello → `false`.
    #[serde(default)]
    pub supports_gzip: bool,
    /// Vestigial: under end-to-end encryption every `msg` payload ships as a
    /// sealed `{enc:"box"}` text envelope, so the old raw-binary transport this
    /// flag once gated is gone. Still parsed from `client_hello` and surfaced in
    /// status for backward compatibility; no code path reads it for transport.
    #[serde(default)]
    pub supports_binary: bool,
    /// Whether this client understands the `sessions_delta` frame (keyed
    /// upsert/remove instead of a whole-list re-push). Gates whether
    /// `publish_sessions` emits deltas: like `supports_gzip`, a single client
    /// that can't apply deltas forces full snapshots for the whole channel
    /// (the relay broadcasts one frame to all). Absent in a hello → `false`,
    /// so an older client keeps receiving full snapshots unchanged.
    #[serde(default)]
    pub supports_delta: bool,
    /// Short git commit the client's bundle was built from (mobile-web
    /// `__APP_COMMIT__`, reported in `client_hello`). Surfaced so the desktop UI
    /// can compare it against its own build commit and flag a phone running a
    /// stale deploy. `None` when the client predates this field or built without
    /// a commit source (e.g. the Harmony native client, which doesn't report it)
    /// — the desktop then shows no version and raises no stale flag.
    #[serde(default)]
    pub app_commit: Option<String>,
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

/// Session ids we've already spawned this process, mapped to their pid. Guards
/// the idempotent-resend case: the phone resends `spawn_session` with the same
/// preassigned `session_id` when the reply frame was lost (relay delivery is
/// best-effort, see fleet-relay `registry::forward`). Without this a resend would
/// launch a second `claude --session-id S`. Keyed on the phone-preassigned id;
/// only recorded when the tool actually honored it (see `record_if_dedupable`).
static SPAWNED_SESSIONS: Mutex<Option<HashMap<String, u32>>> = Mutex::new(None);

/// Prior pid for an already-spawned preassigned session id, or `None` if this id
/// has not been spawned in this process.
fn spawned_session_pid(id: &str) -> Option<u32> {
    SPAWNED_SESSIONS
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|m| m.get(id).copied())
}

/// Record a spawn so a later resend of the same preassigned id is deduped.
/// Only records when the tool honored the phone-preassigned id
/// (`resp_session_id == preassigned`): claude threads `--session-id` through, so
/// the two match and the resend is recoverable; codex mints its own thread id and
/// ignores the preassigned one, so there is no stable key to dedup on and we must
/// not record (a resend of a codex spawn is not recoverable by preassigned id —
/// same contract `serve_spawn_session` documents).
fn record_if_dedupable(preassigned: Option<&str>, resp_session_id: Option<&str>, pid: u32) {
    let Some(id) = preassigned.map(str::trim).filter(|s| !s.is_empty()) else {
        return;
    };
    // Only record when the tool honored the preassigned id: claude threads
    // `--session-id` so resp == preassigned; codex mints its own id (resp != id)
    // and is not dedupable by preassigned key.
    if resp_session_id != Some(id) {
        return;
    }
    SPAWNED_SESSIONS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(id.to_string(), pid);
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Decision ids whose answer we've already delivered this process, mapped to when
/// (ms). Guards the idempotent-resend case for the robust `decision_answer` req:
/// the phone now resends the answer when its reply frame was lost (same
/// best-effort relay as spawn). Without this, a resend of a *live* card whose
/// response file the producer already consumed would get `write_response`'s
/// "no pending request" Err — indistinguishable from "never landed" — and a
/// resend of a *parked* card racing the first `discard` could `claude --resume`
/// twice. Short-circuiting on this map makes both a clean idempotent Ok. Keyed on
/// the decision id (a uuid); pruned by age so the map can't grow unbounded.
static ANSWERED_DECISIONS: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// How long a delivered decision id stays in the dedup map — comfortably longer
/// than the phone's bounded resend window, short enough to self-clean.
const ANSWER_DEDUP_TTL_MS: u64 = 5 * 60 * 1000;

/// Whether `id`'s answer was already delivered recently (a dedup hit). Prunes
/// expired entries as a side effect so the map stays bounded.
fn decision_already_answered(id: &str) -> bool {
    let now = now_ms();
    let mut guard = ANSWERED_DECISIONS.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    map.retain(|_, at| now.saturating_sub(*at) < ANSWER_DEDUP_TTL_MS);
    map.contains_key(id)
}

/// Record that `id`'s answer was delivered, so a later resend short-circuits.
fn record_decision_answered(id: &str) {
    if id.trim().is_empty() {
        return;
    }
    ANSWERED_DECISIONS
        .lock()
        .unwrap()
        .get_or_insert_with(HashMap::new)
        .insert(id.to_string(), now_ms());
}

/// Whether a `resume_session` for `session_id` was already dispatched within the
/// dedup window (a resend hit). See [`serve_resume_session`] for why this exists.
/// **P1 stub — real body lands in P2.** Currently returns `false` always, which
/// is the pre-fix behavior (no resume dedup at all); the P1 test asserts a hit
/// after `record_resume_dispatched`, so this stub makes that test fail red.
fn resume_recently_dispatched(_session_id: &str) -> bool {
    false
}

/// Record that a `resume_session` for `session_id` was dispatched, so a weak-net
/// resend within the window short-circuits. **P1 stub — real body lands in P2.**
fn record_resume_dispatched(_session_id: &str) {}

/// Deliver a mobile answer with idempotent-resend dedup, shared by the legacy
/// fire-and-forget `answer` event and the robust `decision_answer` req. A resend
/// for an id already delivered this process is a no-op success, so an old client
/// and a new client (or a new client's own retries) can't double-deliver.
fn deliver_decision_answer_deduped(payload: &Value) -> Result<(), String> {
    let id = payload.get("id").and_then(Value::as_str).unwrap_or("").trim().to_string();
    if !id.is_empty() && decision_already_answered(&id) {
        return Ok(());
    }
    deliver_decision_answer(payload)?;
    record_decision_answered(&id);
    Ok(())
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
    let supports_gzip =
        payload.get("supportsGzip").and_then(Value::as_bool).unwrap_or(false);
    let supports_binary =
        payload.get("supportsBinary").and_then(Value::as_bool).unwrap_or(false);
    let supports_delta =
        payload.get("supportsDelta").and_then(Value::as_bool).unwrap_or(false);
    // Treat an empty/"unknown" commit as absent so the desktop shows nothing
    // rather than a bogus version for a bundle built without a commit source.
    let app_commit = payload
        .get("appCommit")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != "unknown")
        .map(str::to_string);
    let mut guard = CLIENTS_REGISTRY.lock().unwrap();
    let map = guard.get_or_insert_with(HashMap::new);
    let entry = map.entry(client_id.to_string()).or_insert_with(|| MobileClientInfo {
        client_id: client_id.to_string(),
        label: label.clone(),
        platform: platform.clone(),
        push_subscribed,
        connected_at_ms: now,
        last_seen_ms: now,
        supports_gzip,
        supports_binary,
        supports_delta,
        app_commit: app_commit.clone(),
    });
    entry.label = label;
    entry.platform = platform;
    entry.push_subscribed = push_subscribed;
    entry.supports_gzip = supports_gzip;
    entry.supports_binary = supports_binary;
    entry.supports_delta = supports_delta;
    entry.app_commit = app_commit;
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
static OUT_TX: Mutex<Option<tokio::sync::mpsc::UnboundedSender<Outbound>>> = Mutex::new(None);
/// The AES-256-GCM key derived (via `relay_crypto::derive_keys`) from the
/// pairing secret at connect time and used to seal/open every `msg` payload.
/// Set in `ws_connect_once`, cleared on disconnect. `encode_payload` falls back
/// to deriving from the config file when this is unset (a send racing connect).
static ENC_KEY: Mutex<Option<[u8; 32]>> = Mutex::new(None);

/// A frame queued for the relay. Always a JSON text frame `{type,payload}`;
/// under end-to-end encryption every `msg` payload is a sealed `{enc:"box",…}`
/// envelope, so the raw-binary transport the relay could forward verbatim is no
/// longer used (a gzip blob is not ciphertext — it would leak to the relay).
#[derive(Debug, Clone)]
enum Outbound {
    Text(String),
}
static SESSIONS_LAST_SENT: Mutex<Option<std::time::Instant>> = Mutex::new(None);

/// Host-supplied source for the *current* full sessions snapshot, used to push
/// state to a phone the moment it connects instead of waiting for the next
/// scan-driven change. The desktop `LocalBackend` registers this (see
/// `local_backend.rs`); `fleet serve` leaves it unset because its broadcast
/// loop already force-pushes on a new-client presence bump. When unset, the
/// on-connect push is a no-op and the old "wait for the next scan tick"
/// behaviour applies — which is exactly what the serve loop covers.
#[allow(clippy::type_complexity)]
static SNAPSHOT_PROVIDER: Mutex<Option<Box<dyn Fn() -> Option<Value> + Send + Sync>>> =
    Mutex::new(None);

/// Register the current-sessions provider (see [`SNAPSHOT_PROVIDER`]). Called
/// once at backend startup; a later call replaces the previous provider.
pub fn set_snapshot_provider<F>(f: F)
where
    F: Fn() -> Option<Value> + Send + Sync + 'static,
{
    *SNAPSHOT_PROVIDER.lock().unwrap() = Some(Box::new(f));
}

/// Push the current snapshot to clients right now, bypassing the change-dedup
/// (the caller just reset [`SESSIONS_LAST_HASH`], so a phone that connected
/// mid-idle still gets state even though nothing changed for existing clients).
/// No-op when no provider is registered — see [`SNAPSHOT_PROVIDER`].
fn push_snapshot_on_connect() {
    let snapshot = {
        let guard = SNAPSHOT_PROVIDER.lock().unwrap();
        match guard.as_ref() {
            Some(provider) => provider(),
            None => None,
        }
    };
    if let Some(v) = snapshot {
        publish_sessions(&v);
    }
}

fn send_raw(frame: String) {
    send_out(Outbound::Text(frame));
}

fn send_out(frame: Outbound) {
    if let Some(tx) = OUT_TX.lock().unwrap().as_ref() {
        let _ = tx.send(frame);
    }
}

fn build_msg_frame(payload: Value) -> String {
    json!({ "type": "msg", "payload": payload }).to_string()
}

/// The AES-256-GCM key for this connection. Uses the key cached at connect time;
/// falls back to deriving from the config file for the rare case of a send that
/// races the connect handshake (the frame is dropped by `send_out` anyway when
/// the socket isn't up, so the fallback only ever seals a frame that lands).
fn relay_enc_key() -> [u8; 32] {
    if let Some(k) = *ENC_KEY.lock().unwrap() {
        return k;
    }
    crate::relay_crypto::derive_keys(&load_config().secret).enc_key
}

/// Whether to gzip a `msg` payload before sealing it: only when it's big enough
/// to be worth it AND every live client can inflate (native `DecompressionStream`).
/// Compression must happen *before* encryption — ciphertext does not compress.
fn should_gzip(raw_len: usize, all_support_gzip: bool) -> bool {
    raw_len >= SESSIONS_COMPRESS_MIN_BYTES && all_support_gzip
}

/// Encode a `msg` payload for the wire: (optionally) gzip, then seal under the
/// connection's AES-256-GCM key into the `{enc:"box",iv,ct,z?}` envelope the
/// relay forwards verbatim without ever seeing the plaintext. `z:true` marks a
/// payload that was gzipped before sealing, so the peer inflates after opening.
/// The dedup/hash concerns of callers run on the uncompressed payload, so this
/// stays a pure transport concern. Falls back to sealing the raw bytes if gzip
/// itself fails — never to plaintext (that would defeat the encryption).
fn encode_payload(payload: &Value) -> Outbound {
    let raw = payload.to_string();
    let (bytes, gz) = if should_gzip(raw.len(), all_clients_support_gzip()) {
        match gzip_bytes(raw.as_bytes()) {
            Some(gz) => (gz, true),
            None => (raw.into_bytes(), false),
        }
    } else {
        (raw.into_bytes(), false)
    };
    let sealed = crate::relay_crypto::seal(&relay_enc_key(), &bytes);
    let mut env = json!({ "enc": "box", "iv": sealed.iv, "ct": sealed.ct });
    if gz {
        env["z"] = Value::Bool(true);
    }
    Outbound::Text(build_msg_frame(env))
}

/// Decode an inbound `msg` payload: open the sealed `{enc:"box",…}` envelope,
/// inflate if `z`, and parse the JSON business payload. Returns `None` (frame
/// dropped) for anything that isn't a well-formed sealed envelope — under the
/// hard cutover a peer that can't encrypt simply can't talk to us, so there is
/// no plaintext fallback to be downgraded to.
fn decode_inbound_payload(payload: &Value) -> Option<Value> {
    let sealed: crate::relay_crypto::SealedBox = serde_json::from_value(payload.clone()).ok()?;
    if sealed.enc != "box" {
        return None;
    }
    let plain = match crate::relay_crypto::open(&relay_enc_key(), &sealed) {
        Ok(p) => p,
        Err(e) => {
            crate::log_debug(&format!("[mobile-relay] drop unopenable msg: {e}"));
            return None;
        }
    };
    let json = if payload.get("z").and_then(Value::as_bool).unwrap_or(false) {
        gunzip_bytes(&plain)?
    } else {
        plain
    };
    serde_json::from_slice(&json).ok()
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
    send_out(encode_payload(&build_decision_created_payload(kind, request)));
    send_raw(build_notify_frame(notify_title, notify_body, &tag));
}

/// Tell mobile clients a decision disappeared (answered elsewhere / timed out).
pub fn publish_decision_resolved(kind: &str, id: &str) {
    send_out(encode_payload(&build_decision_resolved_payload(kind, id)));
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
    // Human rename / agent-set semantic title. Load-bearing for Codex sessions:
    // they have no local ai_title, so `fleet__set_session_title` writes here and
    // this is the only real title the phone can render. Without it the phone
    // falls back to the raw first-prompt slug.
    "titleOverride",
    "status",
    "isSubagent",
    // Subagent drill-down: the phone keeps subagent rows (id `agent-<uuid>`) in
    // its session array purely as a lookup table for "打开子代理" navigation, so
    // it needs the parent link to attach them and the type/count to label them.
    // The Tasks list still hides them via `!isSubagent`, so these never surface
    // as tasks. Absent (skipped) on every non-subagent row.
    "parentSessionId",
    "agentType",
    "runningSubagentCount",
    // Ground truth for "Fleet spawned this" — the phone's Tasks list ANDs it with
    // the entrypoint check so a `claude -p` child that only inherited a Fleet
    // CLAUDE_CODE_ENTRYPOINT stops showing up as a task (mirrors desktop).
    "fleetSpawned",
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

/// Byte size a re-encoded decision-asset image is squeezed toward. Every image
/// the mobile client displays is pushed under this so the relay hop stays cheap
/// regardless of the source size (a full-res 3-5 MiB AI preview can't be assumed
/// to cross the relay).
const DECISION_ASSET_TARGET_BYTES: usize = 50 * 1024;
/// Hard ceiling we never want a re-encoded asset to exceed. In practice the
/// shrink loop reaches [`DECISION_ASSET_TARGET_BYTES`] long before this; it only
/// documents the worst-case guarantee for a pathologically incompressible image.
const DECISION_ASSET_HARD_CAP_BYTES: usize = 100 * 1024;
/// JPEG quality steps, tried high→low at each resolution before shrinking.
const DECISION_ASSET_QUALITY_LADDER: [u8; 7] = [90, 80, 70, 60, 50, 40, 30];
/// Never shrink the longest edge below this — past it the asset is too small to
/// be worth showing and further shrinking buys almost nothing.
const DECISION_ASSET_MIN_DIM: u32 = 320;

/// Squeeze a decision-asset image toward [`DECISION_ASSET_TARGET_BYTES`] so it
/// survives the mobile relay hop, returning `(bytes, mime)`. Unlike a simple
/// cap, *every* decodable image is re-encoded — small ones too — because the
/// relay frame budget cares about absolute size, not how large the source was.
///
/// Strategy: at the source resolution, step JPEG quality high→low until an
/// encode fits the target; if even the lowest quality overshoots, shrink the
/// longest edge ~20% and retry, down to [`DECISION_ASSET_MIN_DIM`]. If we bottom
/// out without hitting the target, the smallest encode produced is returned
/// (best effort — a 320px JPEG at the lowest quality is far under the hard cap).
/// Anything that fails to decode (unknown/vector format) is returned untouched
/// so the caller's frame guard still applies.
fn downscale_decision_asset(bytes: Vec<u8>, mime: &str) -> (Vec<u8>, String) {
    downscale_image(
        bytes,
        mime,
        DECISION_ASSET_TARGET_BYTES,
        DECISION_ASSET_HARD_CAP_BYTES,
        DECISION_ASSET_MIN_DIM,
    )
}

/// The shared shrink loop behind [`downscale_decision_asset`] and the
/// transcript thumbnails: step JPEG quality high→low at the current
/// resolution, shrink the longest edge ~20% when even the lowest quality
/// overshoots `target`, and never return over `hard_cap` while `min_dim`
/// still allows shrinking. Undecodable input is returned untouched.
fn downscale_image(
    bytes: Vec<u8>,
    mime: &str,
    target: usize,
    hard_cap: usize,
    min_dim: u32,
) -> (Vec<u8>, String) {
    let Ok(img) = image::load_from_memory(&bytes) else {
        // Undecodable (e.g. a vector/unknown format) — nothing to re-encode; hand
        // the original back and let the caller's frame guard decide.
        return (bytes, mime.to_string());
    };
    let mut work = img;
    // Smallest encode seen so far — returned if no resolution/quality combo
    // reaches the target. Never stays None: the ladder runs at least once.
    let mut best: Option<Vec<u8>> = None;
    loop {
        for &quality in &DECISION_ASSET_QUALITY_LADDER {
            let Some(encoded) = encode_jpeg(&work, quality) else { continue };
            if best.as_ref().map_or(true, |b| encoded.len() < b.len()) {
                best = Some(encoded.clone());
            }
            if encoded.len() <= target {
                return (encoded, "image/jpeg".to_string());
            }
        }
        // Even the lowest quality overshot the target at this resolution. Stop
        // once we're below the min dimension *and* under the hard cap; but if a
        // pathological image is still over the hard cap, keep shrinking past the
        // floor until it fits, so the cap guarantee always holds.
        let longest = work.width().max(work.height());
        let under_cap = best.as_ref().map_or(false, |b| b.len() <= hard_cap);
        if (longest <= min_dim && under_cap) || longest <= 1 {
            break;
        }
        let nw = (work.width() * 4 / 5).max(1);
        let nh = (work.height() * 4 / 5).max(1);
        work = work.resize(nw, nh, image::imageops::FilterType::Lanczos3);
    }
    match best {
        Some(out) => (out, "image/jpeg".to_string()),
        // encode_jpeg never succeeded (should not happen) — fall back to original.
        None => (bytes, mime.to_string()),
    }
}

/// Re-encode `img` as an opaque RGB JPEG at `quality` (0-100). JPEG can't carry
/// alpha, so the image is flattened to RGB first. Returns None only on an
/// encoder error.
fn encode_jpeg(img: &image::DynamicImage, quality: u8) -> Option<Vec<u8>> {
    let rgb = img.to_rgb8();
    let mut out = Vec::new();
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(
        std::io::Cursor::new(&mut out),
        quality,
    );
    encoder
        .encode(rgb.as_raw(), rgb.width(), rgb.height(), image::ExtendedColorType::Rgb8)
        .ok()?;
    Some(out)
}

/// Most-recently-active sessions kept in the mobile snapshot. Sized comfortably
/// above the number of Fleet-owned sessions a busy workspace accumulates inside
/// the scanner's 7-day window, so the cap never crops the task list the mobile
/// client would otherwise render (fields are whitelisted + previews truncated,
/// so even 500 rows stay a small payload).
const SNAPSHOT_MAX_SESSIONS: usize = 500;
/// `lastMessagePreview` cap (chars) — the list row clamps to two lines anyway.
const SNAPSHOT_PREVIEW_CHARS: usize = 160;

/// Slim a full desktop sessions snapshot down to what the mobile client
/// renders: drop subagents and non-Fleet-owned sessions, keep the most recently
/// active [`SNAPSHOT_MAX_SESSIONS`], whitelist fields, truncate previews.
///
/// The Fleet-owned filter mirrors the mobile client's own
/// `isFleetOwnedEntrypoint` gate (TasksView / NewSessionSheet only ever render
/// Fleet-launched sessions) and MUST run *before* the truncation: applying the
/// cap first let external VS Code / bare-CLI sessions occupy slots that the
/// client then discarded, cropping the visible task list below the cap and
/// making the count diverge from the desktop's.
pub fn slim_sessions_snapshot(sessions: &Value) -> Value {
    let Some(list) = sessions.as_array() else {
        return Value::Array(Vec::new());
    };
    let is_subagent =
        |s: &&Value| s.get("isSubagent").and_then(Value::as_bool).unwrap_or(false);
    // Main (non-subagent) Fleet-owned sessions: sorted most-recent-first and
    // capped, exactly as before. The cap is spent only on tasks the phone can
    // actually show — subagents must not evict a visible task from a slot.
    let mut mains: Vec<&Value> = list
        .iter()
        .filter(|s| !is_subagent(s))
        .filter(|s| {
            crate::session_launch::is_fleet_owned_entrypoint(
                s.get("entrypoint").and_then(Value::as_str),
            )
        })
        .collect();
    mains.sort_by_key(|s| {
        std::cmp::Reverse(s.get("lastActivityMs").and_then(Value::as_u64).unwrap_or(0))
    });
    mains.truncate(SNAPSHOT_MAX_SESSIONS);

    // Append subagent rows whose parent survived the cap, so the phone can drill
    // into them (`agent-<uuid>` lookup) without them ever leaking into the task
    // list. `scan.rs` flattens direct + workflow-fanout subagents to point their
    // `parentSessionId` at the owning main session, so matching against kept
    // main ids covers every subagent — there is no deeper nesting to chase.
    let kept_ids: std::collections::HashSet<&str> =
        mains.iter().filter_map(|s| s.get("id").and_then(Value::as_str)).collect();
    let subagents = list.iter().filter(is_subagent).filter(|s| {
        s.get("parentSessionId")
            .and_then(Value::as_str)
            .is_some_and(|p| kept_ids.contains(p))
    });

    let slimmed: Vec<Value> = mains
        .iter()
        .copied()
        .chain(subagents)
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

// ── Transcript slimming (`tail` / `tail_delta`) ───────────────────────────────
//
// Raw transcript records carry far more than the phone renders: embedded
// screenshots (base64, which gzip can't shrink), whole-file `toolUseResult`
// bodies, token usage, cwd/sessionId/parentUuid bookkeeping. On one real
// session that made a single `tail` reply 2.4 MB — 1.7 MB even gzipped — while
// the client's `RawMessage` doesn't even declare `toolUseResult`, and an
// `image` block renders as the literal string "[图片]". The phone downloaded
// megabytes, inflated them, and dropped them on the floor (on a 15s request
// timeout). Slimming to the rendered field set takes that reply to ~13 KB.
//
// These whitelists mirror the mobile client's `RawMessage` / `ContentBlock`
// (mobile-web/src/types.ts) and `toolSummary` (SessionDetailView.tsx). Keep
// them in sync: a field the UI starts rendering must be added here, or it will
// arrive `undefined`.

/// Top-level record fields the mobile `RawMessage` declares. `uuid` is
/// load-bearing beyond rendering — `appendUnique` dedups tailed lines by it.
/// `isMeta`/`sourceToolUseID` drive the client's MetaFoldCard grouping.
const TAIL_MSG_FIELDS: [&str; 7] = [
    "type",
    "uuid",
    "timestamp",
    "isSidechain",
    "isCompactSummary",
    "isMeta",
    "sourceToolUseID",
];
/// Content-block fields the mobile `ContentBlock` declares and renders.
/// `is_error` feeds the tool chip's error badge; result bodies stay stripped.
const TAIL_BLOCK_FIELDS: [&str; 6] = ["text", "thinking", "name", "id", "tool_use_id", "is_error"];
/// The only `message.usage` counters the UI renders (one `↑in ↓out` line per
/// turn); cache bookkeeping counters stay stripped.
const TAIL_USAGE_FIELDS: [&str; 2] = ["input_tokens", "output_tokens"];

/// Transcript-thumbnail budget. Much tighter than a decision asset: thumbs ride
/// inline in the skeleton stream (which is otherwise ~KB-scale) and render at
/// list width, so a ~256px JPEG is plenty. Tap-to-view fetches the original on
/// demand instead.
const TAIL_THUMB_TARGET_BYTES: usize = 12 * 1024;
const TAIL_THUMB_HARD_CAP_BYTES: usize = 20 * 1024;
const TAIL_THUMB_MIN_DIM: u32 = 256;
/// Result payloads can carry several screenshots; cap how many thumb up so one
/// record can't stack unbounded thumbnails into the stream.
const TAIL_THUMB_MAX_PER_RESULT: usize = 3;

/// Thumbnail one base64 image for the skeleton stream, returning the JPEG as
/// base64. `None` means "not worth shipping" (undecodable, or the re-encode
/// couldn't get under the hard cap) — the block then slims to its old
/// type-only form.
///
/// Re-encoding runs inside every `tail` reply, and pagination re-requests the
/// full window, so results are memoised by a hash of the source base64. The
/// cache is process-wide and bounded: on overflow it is simply cleared (a
/// transcript window holds far fewer distinct images than the bound, so a
/// clear-and-refill is rare and cheap). Failures are cached too — a corrupt
/// blob shouldn't be re-decoded on every poll.
fn tail_thumb_from_base64(media_type: &str, data: &str) -> Option<String> {
    use base64::Engine as _;
    use std::collections::HashMap;
    use std::hash::{Hash, Hasher};
    use std::sync::Mutex;

    const CACHE_MAX: usize = 64;
    static CACHE: Mutex<Option<HashMap<u64, Option<String>>>> = Mutex::new(None);

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    data.hash(&mut hasher);
    let key = hasher.finish();

    if let Some(cached) = CACHE
        .lock()
        .ok()
        .and_then(|guard| guard.as_ref().and_then(|m| m.get(&key).cloned()))
    {
        return cached;
    }

    let thumb = base64::engine::general_purpose::STANDARD
        .decode(data)
        .ok()
        .and_then(|bytes| {
            let (out, mime) = downscale_image(
                bytes,
                media_type,
                TAIL_THUMB_TARGET_BYTES,
                TAIL_THUMB_HARD_CAP_BYTES,
                TAIL_THUMB_MIN_DIM,
            );
            // `downscale_image` echoes undecodable input back unchanged — only a
            // real JPEG re-encode under the cap is worth putting on the wire.
            (mime == "image/jpeg" && out.len() <= TAIL_THUMB_HARD_CAP_BYTES)
                .then(|| base64::engine::general_purpose::STANDARD.encode(out))
        });

    if let Ok(mut guard) = CACHE.lock() {
        let map = guard.get_or_insert_with(HashMap::new);
        if map.len() >= CACHE_MAX {
            map.clear();
        }
        map.insert(key, thumb.clone());
    }
    thumb
}

/// Extract a base64 image `source` from a content block, if present.
fn base64_image_source(block: &Value) -> Option<(&str, &str)> {
    let source = block.get("source")?;
    if source.get("type").and_then(Value::as_str) != Some("base64") {
        return None;
    }
    let media_type = source.get("media_type").and_then(Value::as_str).unwrap_or("image/png");
    let data = source.get("data").and_then(Value::as_str)?;
    Some((media_type, data))
}
/// The only `tool_use.input` keys the UI reads — `toolSummary` shows exactly one
/// of these on the tool chip; everything else in `input` is dead weight (a Write
/// tool's whole file body lives there).
const TAIL_TOOL_INPUT_FIELDS: [&str; 7] =
    ["command", "file_path", "pattern", "path", "query", "url", "skill"];

/// Digest a record's `toolUseResult` into a flat, few-dozen-byte stat object
/// for the tool chip's header (diff ± counts, match counts, subagent totals…),
/// so the phone can show the desktop's stat chips without downloading result
/// bodies. Field names mirror the shapes catalogued in the desktop's
/// `toolResults.ts`; `toolUseResult` is an internal Claude Code payload whose
/// keys differ per tool, so every probe is shape-based and optional. Errored
/// calls degrade `toolUseResult` to a bare string — no digest, the block's
/// `is_error` bit already tells the story.
fn tool_result_digest(meta: &Value) -> Option<Value> {
    let obj = meta.as_object()?;
    let mut d = Map::new();
    // Edit / Write: unified-diff hunks → ±line counts.
    if let Some(hunks) = obj.get("structuredPatch").and_then(Value::as_array) {
        let (mut added, mut removed) = (0u64, 0u64);
        for lines in hunks.iter().filter_map(|h| h.get("lines").and_then(Value::as_array)) {
            for line in lines.iter().filter_map(Value::as_str) {
                match line.as_bytes().first() {
                    Some(b'+') => added += 1,
                    Some(b'-') => removed += 1,
                    _ => {}
                }
            }
        }
        d.insert("added".into(), added.into());
        d.insert("removed".into(), removed.into());
    }
    // Bash: stdout/stderr line counts + interrupted flag (no exit code exists).
    if let (Some(stdout), Some(stderr)) = (
        obj.get("stdout").and_then(Value::as_str),
        obj.get("stderr").and_then(Value::as_str),
    ) {
        let count = |s: &str| s.lines().filter(|l| !l.trim().is_empty()).count() as u64;
        d.insert("stdoutLines".into(), count(stdout).into());
        d.insert("stderrLines".into(), count(stderr).into());
        if obj.get("interrupted") == Some(&Value::Bool(true)) {
            d.insert("interrupted".into(), true.into());
        }
    }
    // Agent subagent run totals.
    if let Some(status) = obj.get("status").and_then(Value::as_str) {
        d.insert("agentStatus".into(), status.into());
        // The subagent's session id (`agent-<uuid>` is scanned from `agentId`);
        // it's the only handle the phone's "打开子代理" button has to look the
        // subagent up in the session array and drill into its transcript.
        if let Some(id) = obj.get("agentId").and_then(Value::as_str) {
            d.insert("agentId".into(), id.into());
        }
        for (src, dst) in [
            ("totalDurationMs", "durationMs"),
            ("totalTokens", "tokens"),
            ("totalToolUseCount", "toolUses"),
        ] {
            if let Some(v) = obj.get(src).filter(|v| v.is_number()) {
                d.insert(dst.into(), v.clone());
            }
        }
    }
    // Grep / Glob search stats.
    if let Some(files) = obj.get("numFiles").filter(|v| v.is_number()) {
        d.insert("files".into(), files.clone());
        if let Some(m) = obj
            .get("numMatches")
            .or_else(|| obj.get("totalMatches"))
            .filter(|v| v.is_number())
        {
            d.insert("matches".into(), m.clone());
        }
        if obj.get("truncated") == Some(&Value::Bool(true)) {
            d.insert("truncated".into(), true.into());
        }
    }
    // WebSearch: result-link count.
    if let Some(links) = obj.get("links").and_then(Value::as_array) {
        d.insert("links".into(), (links.len() as u64).into());
    }
    // WebFetch: HTTP status + payload size.
    if obj.get("url").is_some() {
        if let Some(code) = obj.get("code").filter(|v| v.is_number()) {
            d.insert("httpCode".into(), code.clone());
        }
        if let Some(bytes) = obj.get("bytes").filter(|v| v.is_number()) {
            d.insert("bytes".into(), bytes.clone());
        }
    }
    // TodoWrite: done/total of the new checklist.
    if let Some(todos) = obj.get("newTodos").and_then(Value::as_array) {
        let done = todos
            .iter()
            .filter(|t| t.get("status").and_then(Value::as_str) == Some("completed"))
            .count() as u64;
        d.insert("todoDone".into(), done.into());
        d.insert("todoTotal".into(), (todos.len() as u64).into());
    }
    if d.is_empty() { None } else { Some(Value::Object(d)) }
}

/// Slim one content block to the fields the mobile renders. An `image` block
/// sheds its original base64 `source` — the biggest and least compressible part
/// of a transcript — and carries a server-side ~256px JPEG thumbnail instead
/// (`_thumb: true`); an undecodable image keeps only its `type` and renders as
/// the "[图片]" placeholder. A `tool_result` block's body stays stripped, but
/// any images inside it surface as a capped `_thumbs` list.
fn slim_tail_block(block: &Value) -> Value {
    let Some(obj) = block.as_object() else {
        return block.clone();
    };
    let mut out = Map::new();
    if let Some(ty) = obj.get("type") {
        out.insert("type".into(), ty.clone());
    }
    for key in TAIL_BLOCK_FIELDS {
        if let Some(v) = obj.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    if let Some(input) = obj.get("input").and_then(Value::as_object) {
        let mut slim_input = Map::new();
        for key in TAIL_TOOL_INPUT_FIELDS {
            if let Some(v) = input.get(key) {
                slim_input.insert(key.into(), v.clone());
            }
        }
        if !slim_input.is_empty() {
            out.insert("input".into(), Value::Object(slim_input));
        }
    }
    match obj.get("type").and_then(Value::as_str) {
        Some("image") => {
            if let Some(thumb) = base64_image_source(block)
                .and_then(|(media_type, data)| tail_thumb_from_base64(media_type, data))
            {
                out.insert(
                    "source".into(),
                    json!({ "type": "base64", "media_type": "image/jpeg", "data": thumb }),
                );
                out.insert("_thumb".into(), true.into());
            }
        }
        Some("tool_result") => {
            let thumbs: Vec<Value> = obj
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|b| b.get("type").and_then(Value::as_str) == Some("image"))
                .filter_map(base64_image_source)
                .filter_map(|(media_type, data)| tail_thumb_from_base64(media_type, data))
                .take(TAIL_THUMB_MAX_PER_RESULT)
                .map(Value::from)
                .collect();
            if !thumbs.is_empty() {
                out.insert("_thumbs".into(), Value::Array(thumbs));
            }
        }
        _ => {}
    }
    Value::Object(out)
}

/// Slim one transcript record to the mobile `RawMessage` field set. Row count is
/// preserved (rows the client filters out still ship, just empty-ish): the
/// client's "load more" gate compares `messages.length` against the requested
/// `n`, so dropping rows here would silently kill pagination.
fn slim_tail_message(msg: &Value) -> Value {
    let Some(obj) = msg.as_object() else {
        return msg.clone();
    };
    let mut out = Map::new();
    for key in TAIL_MSG_FIELDS {
        if let Some(v) = obj.get(key) {
            out.insert(key.into(), v.clone());
        }
    }
    if let Some(message) = obj.get("message").and_then(Value::as_object) {
        let mut slim_msg = Map::new();
        if let Some(role) = message.get("role") {
            slim_msg.insert("role".into(), role.clone());
        }
        // Turn metadata for the per-turn usage line and run-termination checks.
        for key in ["model", "stop_reason"] {
            if let Some(v) = message.get(key) {
                slim_msg.insert(key.into(), v.clone());
            }
        }
        if let Some(usage) = message.get("usage").and_then(Value::as_object) {
            let mut slim_usage = Map::new();
            for key in TAIL_USAGE_FIELDS {
                if let Some(v) = usage.get(key) {
                    slim_usage.insert(key.into(), v.clone());
                }
            }
            if !slim_usage.is_empty() {
                slim_msg.insert("usage".into(), Value::Object(slim_usage));
            }
        }
        match message.get("content") {
            // Block list: slim each block. The record-level `toolUseResult`
            // (stripped as a whole) is digested into a tiny `_digest` stat
            // object on the record's `tool_result` block, keyed client-side by
            // `tool_use_id` for the matching tool chip's header.
            Some(Value::Array(blocks)) => {
                let digest = obj.get("toolUseResult").and_then(tool_result_digest);
                let slimmed = blocks.iter().map(|b| {
                    let mut slim = slim_tail_block(b);
                    if let (Some(d), Some("tool_result")) =
                        (&digest, b.get("type").and_then(Value::as_str))
                    {
                        if let Some(o) = slim.as_object_mut() {
                            o.insert("_digest".into(), d.clone());
                        }
                    }
                    slim
                });
                slim_msg.insert("content".into(), Value::Array(slimmed.collect()));
            }
            // Plain string content is rendered verbatim.
            Some(other) => {
                slim_msg.insert("content".into(), other.clone());
            }
            None => {}
        }
        out.insert("message".into(), Value::Object(slim_msg));
    }
    Value::Object(out)
}

/// Slim a list of transcript records for the wire. Used by both `tail` and
/// `tail_delta` — the delta lines are appended to the same list, so slimming
/// only the initial tail would let one fat record re-inflate the transcript.
pub fn slim_tail_messages(msgs: Vec<Value>) -> Vec<Value> {
    msgs.iter().map(slim_tail_message).collect()
}

/// Hash of the last snapshot actually sent; unchanged data is not re-pushed.
/// Reset on client presence changes (see `ws_connect_once`) so a phone that
/// just connected still gets the current state on the next scan tick.
static SESSIONS_LAST_HASH: AtomicU64 = AtomicU64::new(0);

/// Per-session baseline: `id → hash of that session's slim JSON` for the last
/// full state every current client holds. `None` means "no baseline" — the next
/// push must be a full `sessions` snapshot (a fresh/reconnected client has no
/// state to diff against). Reset to `None` alongside [`SESSIONS_LAST_HASH`] at
/// every presence change. When present *and* every client supports deltas,
/// `publish_sessions` emits a `sessions_delta{upsert,remove}` computed against
/// this map instead of re-sending the whole list.
static SESSIONS_BASELINE: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// Clear both the whole-list dedup hash and the per-session delta baseline, so
/// the next `publish_sessions` re-sends a full snapshot and rebuilds the
/// baseline. Called at every presence change (new socket / client joined).
fn reset_sessions_dedup() {
    SESSIONS_LAST_HASH.store(0, Ordering::SeqCst);
    *SESSIONS_BASELINE.lock().unwrap() = None;
}

/// Per-`id` hash of each slim session object — the baseline a later delta diffs
/// against. Sessions without a string `id` are skipped (they can't be keyed).
fn per_id_hashes(slim: &Value) -> HashMap<String, u64> {
    let mut m = HashMap::new();
    if let Some(arr) = slim.as_array() {
        for s in arr {
            if let Some(id) = s.get("id").and_then(Value::as_str) {
                m.insert(id.to_string(), snapshot_hash(s));
            }
        }
    }
    m
}

/// Diff the current slim snapshot against the baseline, keyed by session `id`.
/// Returns `(upsert, remove)`: `upsert` holds the full slim object for every
/// session that is new or whose content hash changed; `remove` holds the ids
/// present in the baseline but absent from the current snapshot (dropped, or
/// fell out of the top-[`SNAPSHOT_MAX_SESSIONS`] cap — both render as a removal
/// on the phone). Order within `upsert` follows the snapshot (already sorted by
/// `lastActivityMs` desc); the client re-sorts its merged map regardless.
fn diff_snapshot(prev: &HashMap<String, u64>, slim: &Value) -> (Vec<Value>, Vec<String>) {
    let mut upsert = Vec::new();
    let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
    if let Some(arr) = slim.as_array() {
        for s in arr {
            let Some(id) = s.get("id").and_then(Value::as_str) else { continue };
            seen.insert(id);
            if prev.get(id) != Some(&snapshot_hash(s)) {
                upsert.push(s.clone());
            }
        }
    }
    let remove: Vec<String> =
        prev.keys().filter(|id| !seen.contains(id.as_str())).cloned().collect();
    (upsert, remove)
}

fn snapshot_hash(v: &Value) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    v.to_string().hash(&mut h);
    // Never collide with the "nothing sent yet" sentinel 0.
    h.finish() | 1
}

/// Below this serialized size, gzip costs more (per-frame gzip header) than it
/// saves, so small payloads are sealed uncompressed.
const SESSIONS_COMPRESS_MIN_BYTES: usize = 4096;

/// Whether every live client can inflate a gzipped payload (native
/// `DecompressionStream`, reported as `supportsGzip` in `client_hello`). A
/// single client that can't (or an empty registry) forces uncompressed sealing
/// for the whole channel — the relay broadcasts one frame to all clients, so we
/// can't compress for some only.
fn all_clients_support_gzip() -> bool {
    let devices = live_devices();
    !devices.is_empty() && devices.iter().all(|d| d.supports_gzip)
}

/// Whether every live client can apply a `sessions_delta` frame (reported as
/// `supportsDelta` in `client_hello`). A single client that can't — or an empty
/// registry — forces full snapshots for the whole channel, since the relay
/// broadcasts one frame to all clients and a delta is meaningless to a client
/// that never received the matching baseline.
fn all_clients_support_delta() -> bool {
    let devices = live_devices();
    !devices.is_empty() && devices.iter().all(|d| d.supports_delta)
}

/// gzip a byte slice to raw bytes (compressed *before* encryption).
/// Returns `None` if compression itself fails (then the caller seals raw bytes).
fn gzip_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Write as _;
    let mut enc = GzEncoder::new(Vec::new(), Compression::default());
    enc.write_all(bytes).ok()?;
    enc.finish().ok()
}

/// Inflate raw gzip bytes (the plaintext of a sealed payload that was gzipped
/// before encryption). Returns `None` on malformed gzip (the frame is dropped).
fn gunzip_bytes(bytes: &[u8]) -> Option<Vec<u8>> {
    use std::io::Read as _;
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(bytes).read_to_end(&mut out).ok()?;
    Some(out)
}

/// Push a sessions snapshot. Skipped when no client is online (Web Push is
/// pointless for passive data), throttled to one per 2s, slimmed to the
/// mobile field set, and deduplicated against the last pushed content.
///
/// When every live client advertises `supportsDelta` *and* a per-session
/// baseline exists, only the sessions that changed (`upsert`) and the ids that
/// vanished (`remove`) ship, as a `sessions_delta` frame — a busy fleet whose
/// token counts tick every message no longer re-pushes the whole ≤500-row list
/// each time. Otherwise (any legacy client present, or no baseline yet after a
/// (re)connect) a full `sessions` snapshot ships and (re)seeds the baseline.
///
/// Either frame is compressed by `encode_payload` (~110KB→29KB for a full
/// snapshot on this repo's own data); the hash/diff dedup here runs on the
/// uncompressed content, so compression stays a pure transport concern.
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
        return; // whole list unchanged since the last push
    }

    // Decide full vs delta under the baseline lock so a concurrent presence
    // reset can't interleave between the diff and the baseline update.
    let mut baseline = SESSIONS_BASELINE.lock().unwrap();
    let can_delta = all_clients_support_delta() && baseline.is_some();
    let frame = if can_delta {
        let (upsert, remove) = diff_snapshot(baseline.as_ref().unwrap(), &slim);
        *baseline = Some(per_id_hashes(&slim));
        // The whole-list hash changed, so per-id diff normally finds something;
        // guard the degenerate case (e.g. hash collision) by sending nothing.
        if upsert.is_empty() && remove.is_empty() {
            return;
        }
        json!({ "event": "sessions_delta", "upsert": upsert, "remove": remove })
    } else {
        *baseline = Some(per_id_hashes(&slim));
        json!({ "event": "sessions", "sessions": slim })
    };
    drop(baseline);
    send_out(encode_payload(&frame));
}

// ── Inbound: answers and requests from mobile clients ────────────────────────

/// Business frame acknowledging that a write `req` reached the desktop, sent
/// before the final `reply` (方案 A early ack). Carries the same `req_id` so the
/// client can correlate it.
fn ack_frame(req_id: &Value) -> Value {
    json!({ "event": "ack", "req_id": req_id })
}

/// Whether a `req` method warrants an early submit-ack. Write methods (which
/// have side effects and whose "did it land?" the client acts on) do; read
/// methods (idempotent, just retried) don't. Kept in sync with the write-method
/// arm of `serve_request`.
fn is_ackable_method(method: &str) -> bool {
    matches!(
        method,
        "spawn_session"
            | "resume_session"
            | "enqueue_message"
            | "cancel_pending_message"
            | "interrupt"
            | "stop"
            | "stop_workspace"
            | "session_mark"
            | "upload_attachment"
            | "decision_answer"
    )
}

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
        Some("req") => {
            // Early submit-ack: for write methods, tell the client "received, now
            // processing" the instant the req lands, before the (possibly slower)
            // work + final reply. Lets the client confirm its submit reached the
            // desktop without waiting out the whole request timeout. Reads don't
            // ack — they just retry, and the extra frame isn't worth it on hot
            // polling paths. The ack rides the same best-effort relay path as the
            // reply, so it's a latency/feedback win, not a delivery guarantee (the
            // resend + dedup guard in serve_spawn_session covers loss).
            let method = payload.get("method").and_then(Value::as_str).unwrap_or("");
            if is_ackable_method(method) {
                let req_id = payload.get("req_id").cloned().unwrap_or(Value::Null);
                send_out(encode_payload(&ack_frame(&req_id)));
            }
            Some(handle_request(payload))
        }
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

/// Route a mobile answer through `parked::deliver`, exactly as the desktop panel
/// and the probe API do: a live card unblocks the waiting hook/MCP subprocess via
/// its `<id>.response.json`, and a parked one (whose producer is long gone)
/// resumes the session with the answer instead.
fn handle_answer(payload: &Value) -> Result<(), String> {
    deliver_decision_answer_deduped(payload)
}

/// `req`-channel form of a decision answer — the phone's robust path. Unlike the
/// fire-and-forget `answer` event, this rides req/reply so the phone gets a
/// delivery verdict and resends on loss; the dedup in
/// [`deliver_decision_answer_deduped`] is what makes that resend safe. Returns a
/// null reply body on success (the phone only acts on `ok`).
fn serve_decision_answer(params: &Value) -> Result<Value, String> {
    deliver_decision_answer_deduped(params).map(|()| Value::Null)
}

/// Deliver a normalized decision response through the same live/parked path
/// used by mobile and desktop clients. Fleet Cloud Runner calls this boundary
/// after durably claiming a `decision.response` command.
pub fn deliver_decision_answer(payload: &Value) -> Result<(), String> {
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
            let declined = payload.get("declined").and_then(Value::as_bool).unwrap_or(false);
            let resp = crate::elicitation::ElicitationResponse { id: id.clone(), declined, answers };
            crate::parked::deliver(&id, &resp, declined, crate::elicitation::write_response)
        }
        "fleet-ask" => {
            let answers: std::collections::BTreeMap<String, String> = payload
                .get("answers")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| format!("bad answers: {e}"))?
                .unwrap_or_default();
            let cancelled = payload.get("cancelled").and_then(Value::as_bool).unwrap_or(false);
            let resp = crate::mcp_ipc::FleetAskResponse { id: id.clone(), answers, cancelled };
            crate::parked::deliver(&id, &resp, cancelled, crate::mcp_ipc::write_response)
        }
        "plan-approval" => {
            let decision = str_field("decision").ok_or("missing decision")?;
            let resp = crate::plan_approval::PlanApprovalResponse {
                id: id.clone(),
                decision,
                edited_plan: str_field("editedPlan"),
                feedback: str_field("feedback"),
            };
            // `dismissed: false` — a rejection is an answer the agent has to wake
            // up and hear, not a card the user waved away.
            crate::parked::deliver(&id, &resp, false, crate::plan_approval::write_response)
        }
        "a2ui-render" => {
            let action_context: std::collections::BTreeMap<String, String> = payload
                .get("actionContext")
                .cloned()
                .map(serde_json::from_value)
                .transpose()
                .map_err(|e| format!("bad actionContext: {e}"))?
                .unwrap_or_default();
            let cancelled = payload.get("cancelled").and_then(Value::as_bool).unwrap_or(false);
            let resp = crate::mcp_a2ui_ipc::A2uiRenderResponse {
                id: id.clone(),
                action_name: str_field("actionName"),
                action_context,
                cancelled,
            };
            crate::parked::deliver(&id, &resp, cancelled, crate::mcp_a2ui_ipc::write_response)
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

/// Workspace paths of every session the backend knows about — the access
/// envelope the git/repo entry points validate against (same list the
/// `/git_status` HTTP endpoint builds).
fn known_workspaces() -> Vec<String> {
    crate::agent_source::build_sources()
        .iter()
        .flat_map(|s| s.scan_sessions())
        .map(|s| s.workspace_path)
        .collect()
}

fn serve_request(method: &str, params: &Value) -> Result<Value, String> {
    match method {
        // ── Read methods ─────────────────────────────────────────────────
        "pending_snapshot" => serve_pending_snapshot(params),
        "task_plans" => serve_task_plans(params),
        "session_decisions" => serve_session_decisions(params),
        "decision_asset" => serve_decision_asset(params),
        "tail" => serve_tail(params),
        "tail_delta" => serve_tail_delta(params),
        "tool_detail" => serve_tool_detail(params),
        "live_thinking" => serve_live_thinking(params),
        "handoff_chain" => serve_handoff_chain(params),
        "workflow_trees" => serve_workflow_trees(params),
        "token_breakdown" => serve_token_breakdown(params),
        "today_usage" => serve_today_usage(params),
        "today_usage_breakdown" => serve_today_usage_breakdown(params),
        "account_usage" => serve_account_usage(params),
        "usage_history" => serve_usage_history(params),
        "codex_usage_history" => serve_codex_usage_history(params),
        "skill_history" => serve_skill_history(params),
        "guard_analyze" => serve_guard_analyze(params),
        "session_search" => serve_session_search(params),
        "wiki_list" => serve_wiki_list(params),
        "wiki_file" => serve_wiki_file(params),
        "wiki_search" => serve_wiki_search(params),
        "wiki_export" => serve_wiki_export(params),
        "chat_workspace" => serve_chat_workspace(params),
        "sources_config" => serve_sources_config(params),
        "browse_dir" => serve_browse_dir(params),
        // ── Write methods ────────────────────────────────────────────────
        "spawn_session" => serve_spawn_session(params),
        "resume_session" => serve_resume_session(params),
        "enqueue_message" => serve_enqueue_message(params),
        "cancel_pending_message" => serve_cancel_pending_message(params),
        "interrupt" => serve_interrupt(params),
        "stop" => serve_stop(params),
        "stop_workspace" => serve_stop_workspace(params),
        "session_mark" => serve_session_mark(params),
        "upload_attachment" => serve_upload_attachment(params),
        "decision_answer" => serve_decision_answer(params),
        "attachments_exist" => serve_attachments_exist(params),
        "session_read" => serve_session_read(params),
        // ── Repository "仓库" surface ─────────────────────────────────────
        "repo_list" => serve_repo_list(params),
        "repo_detail" => serve_repo_detail(params),
        "repo_push" => serve_repo_push(params),
        "repo_pull" => serve_repo_pull(params),
        other => Err(format!("unknown method: {other}")),
    }
}

// ── Per-method handlers ──────────────────────────────────────────────────
// Each `serve_<name>` holds the verbatim body of its former `match` arm in
// `serve_request` above; the match is now just a method-literal → handler
// table. Handlers that ignore the request body take `_params`.

// Same aggregation as `LocalBackend::list_pending_decisions`, minus
// the session-cache display enrichment — the mobile client resolves
// workspace/title labels from its own `sessions` snapshot.
fn serve_pending_snapshot(_params: &Value) -> Result<Value, String> {
    // Parked cards are pending too — they just live in the parked store
    // rather than the channel's request dir. The phone's snapshot is
    // authoritative (it *replaces* the client's card list), so omitting
    // them here would make a timed-out card vanish from the phone on the
    // very next reconcile — the disappearance this whole mechanism exists
    // to prevent, just on the other screen.
    let read_all = |ids: Vec<String>,
                    read: &dyn Fn(&str) -> Option<Value>,
                    kind: crate::parked::ParkedKind|
     -> Vec<Value> {
        ids.iter()
            .filter_map(|id| read(id))
            .chain(crate::parked::list_requests::<Value>(kind))
            .collect()
    };
    // Guard and permission prompts are never parked (their timeout is a
    // deny, not a pause), so they keep the plain live-only listing.
    let read_live = |ids: Vec<String>, read: &dyn Fn(&str) -> Option<Value>| -> Vec<Value> {
        ids.iter().filter_map(|id| read(id)).collect()
    };
    Ok(json!({
        "guard": read_live(crate::guard::list_pending_requests(), &|id| {
            crate::guard::read_request(id).and_then(|r| serde_json::to_value(r).ok())
        }),
        "elicitation": read_all(
            crate::elicitation::list_pending_requests(),
            &|id| {
                crate::elicitation::read_request(id).and_then(|r| serde_json::to_value(r).ok())
            },
            crate::parked::ParkedKind::Elicitation,
        ),
        "fleetAsk": read_all(
            crate::mcp_ipc::list_pending_requests(),
            &|id| crate::mcp_ipc::read_request(id).and_then(|r| serde_json::to_value(r).ok()),
            crate::parked::ParkedKind::FleetAsk,
        ),
        "planApproval": read_all(
            crate::plan_approval::list_pending_requests(),
            &|id| {
                crate::plan_approval::read_request(id).and_then(|r| serde_json::to_value(r).ok())
            },
            crate::parked::ParkedKind::PlanApproval,
        ),
        "permissionPrompt": read_live(
            crate::permission_prompt_ipc::list_pending_requests(),
            &|id| {
                crate::permission_prompt_ipc::read_request(id)
                    .and_then(|r| serde_json::to_value(r).ok())
            },
        ),
        "a2uiRender": read_all(
            crate::mcp_a2ui_ipc::list_pending_requests(),
            &|id| {
                crate::mcp_a2ui_ipc::read_request(id).and_then(|r| serde_json::to_value(r).ok())
            },
            crate::parked::ParkedKind::A2uiRender,
        ),
    }))
}

fn serve_task_plans(params: &Value) -> Result<Value, String> {
    let workspace = params
        .get("workspacePath")
        .and_then(Value::as_str)
        .ok_or("missing workspacePath")?;
    let session_id = params.get("sessionId").and_then(Value::as_str);
    let plans =
        crate::prd_tasks::list_workspace_task_plans(std::path::Path::new(workspace), session_id);
    serde_json::to_value(plans).map_err(|e| e.to_string())
}

fn serve_session_decisions(params: &Value) -> Result<Value, String> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or("missing sessionId")?;
    let jsonl = params.get("jsonlPath").and_then(Value::as_str).map(std::path::Path::new);
    let records =
        crate::decision_history::list_session_records_with_jsonl(session_id, jsonl);
    serde_json::to_value(records).map_err(|e| e.to_string())
}

fn serve_decision_asset(params: &Value) -> Result<Value, String> {
    use base64::Engine as _;
    let id = params.get("id").and_then(Value::as_str).ok_or("missing id")?;
    let qidx = params.get("qidx").and_then(Value::as_str).ok_or("missing qidx")?;
    let rel = params.get("rel").and_then(Value::as_str).ok_or("missing rel")?;
    let asset = crate::mcp_ipc::read_decision_asset(id, qidx, rel)?;
    // Every image is squeezed toward the target size so the WS frame stays
    // small enough to cross the relay, regardless of the source size.
    let (bytes, mime) = downscale_decision_asset(asset.bytes, &asset.mime);
    // Defensive ceiling: a target-squeezed JPEG is far under this, so this only
    // trips on a pathological asset we could neither decode nor shrink.
    const MAX_ASSET_BYTES: usize = 12 * 1024 * 1024;
    if bytes.len() > MAX_ASSET_BYTES {
        return Err(format!("asset too large: {} bytes", bytes.len()));
    }
    Ok(json!({
        "mime": mime,
        "base64": base64::engine::general_purpose::STANDARD.encode(&bytes),
    }))
}

// Last `n` transcript messages — the mobile SessionDetail polls this
// (same model as the desktop HistoryView tab; no watcher push).
fn serve_tail(params: &Value) -> Result<Value, String> {
    let path = params.get("path").and_then(Value::as_str).ok_or("missing path")?;
    let n = params.get("n").and_then(Value::as_u64).unwrap_or(200) as usize;
    let sources = crate::agent_source::build_sources();
    let source = crate::agent_source::find_source_for_path(&sources, path)
        .ok_or_else(|| format!("no agent source for path: {path}"))?;
    source
        .get_messages_tail(path, n)
        .map(|msgs| Value::Array(slim_tail_messages(msgs)))
}

// Byte-offset incremental tail (mirrors the `/tail` endpoint): the
// client polls with its last `newOffset` and receives only the lines
// appended since. Omitting `offset` locates the current end without
// reading the body — the cheap "start following from here" call.
fn serve_tail_delta(params: &Value) -> Result<Value, String> {
    let path = params.get("path").and_then(Value::as_str).ok_or("missing path")?;
    let sources = crate::agent_source::build_sources();
    let source = crate::agent_source::find_source_for_path(&sources, path)
        .ok_or_else(|| format!("no agent source for path: {path}"))?;
    let resolved = source
        .resolve_file_path(path)
        .ok_or_else(|| format!("cannot resolve path: {path}"))?;
    let size = std::fs::metadata(&resolved).map_err(|e| e.to_string())?.len();
    let offset = params.get("offset").and_then(Value::as_u64);
    let Some(offset) = offset else {
        // Bootstrap: locate the current end so the client follows from here.
        return Ok(json!({ "lines": [], "newOffset": size }));
    };
    // Source-aware incremental follow: Claude gets the byte-offset raw tail
    // (each jsonl line is a self-contained message); Codex re-normalizes the
    // trailing window (its rollout is folded — a raw byte slice renders as
    // nothing), returning messages the client dedups by their stable `uuid`.
    let (lines, new_offset) = source.tail_incremental(path, offset)?;
    Ok(json!({ "lines": slim_tail_messages(lines), "newOffset": new_offset }))
}

/// Screenshots surfaced in one `tool_detail` reply are capped so a
/// screenshot-heavy result can't stack megabytes into a single frame; each is
/// re-encoded at the decision-asset budget (≤100 KB guaranteed).
const TOOL_DETAIL_MAX_IMAGES: usize = 6;

/// On-demand expansion for one tool call — the mobile counterpart of the
/// desktop's `get_tool_result_full`. The skeleton stream ships only summaries
/// and digests (see `slim_tail_messages`); when the reader taps a tool chip
/// open, this returns the full `tool_use.input`, the `tool_result` content and
/// the structured `toolUseResult` for that `tool_use_id`.
///
/// Large string leaves are truncated to transport previews unless `full` is
/// set (`truncated: true` marks a reply the client can re-request with
/// `full`), mirroring the desktop's two-stage expand. Embedded screenshots are
/// never shipped raw: they are pulled out of `content` into an `images` list
/// re-encoded at the decision-asset budget, `full` or not.
///
/// Reads the transcript where the relay host runs, like `serve_tail` — no
/// Backend-trait hop is involved on the mobile path.
fn serve_tool_detail(params: &Value) -> Result<Value, String> {
    use base64::Engine as _;

    let path = params.get("path").and_then(Value::as_str).ok_or("missing path")?;
    let tool_use_id =
        params.get("tool_use_id").and_then(Value::as_str).ok_or("missing tool_use_id")?;
    let full = params.get("full").and_then(Value::as_bool).unwrap_or(false);

    let sources = crate::agent_source::build_sources();
    let source = crate::agent_source::find_source_for_path(&sources, path)
        .ok_or_else(|| format!("no agent source for path: {path}"))?;
    let resolved = source
        .resolve_file_path(path)
        .ok_or_else(|| format!("cannot resolve path: {path}"))?;

    let mut detail = crate::message_trim::extract_full_tool_result(&resolved, tool_use_id)?;
    let obj = detail.as_object_mut().expect("extract_full_tool_result returns an object");

    // Pull screenshots out of the result content before any truncation: a
    // base64 leaf would otherwise be cut into a useless 1 KB preview. The raw
    // source is stripped from the block either way.
    let mut images: Vec<Value> = Vec::new();
    if let Some(blocks) = obj.get_mut("content").and_then(Value::as_array_mut) {
        for block in blocks.iter_mut() {
            if block.get("type").and_then(Value::as_str) != Some("image") {
                continue;
            }
            let source_pair = base64_image_source(block)
                .map(|(media_type, data)| (media_type.to_string(), data.to_string()));
            if let Some((media_type, data)) = source_pair {
                if images.len() < TOOL_DETAIL_MAX_IMAGES {
                    if let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(&data) {
                        let (out, mime) = downscale_image(
                            bytes,
                            &media_type,
                            DECISION_ASSET_TARGET_BYTES,
                            DECISION_ASSET_HARD_CAP_BYTES,
                            DECISION_ASSET_MIN_DIM,
                        );
                        if out.len() <= DECISION_ASSET_HARD_CAP_BYTES {
                            images.push(json!({
                                "media_type": mime,
                                "data": base64::engine::general_purpose::STANDARD.encode(out),
                            }));
                        }
                    }
                }
            }
            if let Some(b) = block.as_object_mut() {
                b.remove("source");
            }
        }
    }
    if !images.is_empty() {
        obj.insert("images".into(), Value::Array(images));
    }

    if !full {
        let mut truncated = false;
        for key in ["content", "toolUseResult", "input"] {
            if let Some(v) = obj.get_mut(key) {
                truncated |= crate::message_trim::truncate_value_for_transport(v);
            }
        }
        if truncated {
            obj.insert("truncated".into(), true.into());
        }
    }
    Ok(detail)
}

// Serializes to `null` when there's no live sidecar for this session.
fn serve_live_thinking(params: &Value) -> Result<Value, String> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or("missing sessionId")?;
    serde_json::to_value(crate::live_thinking::read_live_thinking(session_id))
        .map_err(|e| e.to_string())
}

// `null` when the session isn't on any relay chain.
fn serve_handoff_chain(params: &Value) -> Result<Value, String> {
    let session_id = params
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or("missing sessionId")?;
    serde_json::to_value(crate::handoff::chain_containing(session_id))
        .map_err(|e| e.to_string())
}

fn serve_workflow_trees(params: &Value) -> Result<Value, String> {
    let path = params.get("path").and_then(Value::as_str).ok_or("missing path")?;
    let trees =
        crate::workflow::discover_workflow_trees(std::path::Path::new(path));
    serde_json::to_value(trees).map_err(|e| e.to_string())
}

fn serve_token_breakdown(params: &Value) -> Result<Value, String> {
    let path = params.get("path").and_then(Value::as_str).ok_or("missing path")?;
    let project_root = params.get("projectRoot").and_then(Value::as_str);
    let breakdown = crate::token_analysis::aggregate_task(
        std::path::Path::new(path),
        project_root.map(std::path::Path::new),
    )?;
    serde_json::to_value(breakdown).map_err(|e| e.to_string())
}

// Today's cumulative token/cost counter for the mobile header (mirrors
// `LocalBackend::today_usage` / `/today_usage`).
fn serve_today_usage(_params: &Value) -> Result<Value, String> {
    let sources = crate::agent_source::build_sources();
    let sessions = crate::session::scan_all_sources(&sources);
    let usage = crate::today_usage::today_usage(&sessions);
    serde_json::to_value(usage).map_err(|e| e.to_string())
}

// Per-model receipt breakdown behind the header counter (mirrors
// `LocalBackend::today_usage_breakdown` / `/today_usage_breakdown`).
fn serve_today_usage_breakdown(_params: &Value) -> Result<Value, String> {
    let sources = crate::agent_source::build_sources();
    let sessions = crate::session::scan_all_sources(&sources);
    let breakdown = crate::today_usage::today_usage_breakdown(&sessions);
    serde_json::to_value(breakdown).map_err(|e| e.to_string())
}

// Account + rate-limit usage for the mobile 「账号与用量」 page: the Claude
// account/plan with its 5h/7d bars, plus a normalised bar set for every
// other available source. Runs on a plain thread rather than this ws
// blocking task because every fetch inside builds its own tokio runtime
// (`fetch_account_info_blocking` and friends) — see `account_usage_payload`.
fn serve_account_usage(_params: &Value) -> Result<Value, String> {
    std::thread::spawn(account_usage_payload)
        .join()
        .map_err(|_| "account/usage fetch panicked".to_string())
}

// Occupancy time series behind the mobile page's 24h chart. Pure disk
// read of the snapshots the background sampler persists — no network, so
// it stays on this thread (unlike `account_usage` above). Values are the
// 0–1 fraction; the window defaults to the last 24h.
fn serve_usage_history(params: &Value) -> Result<Value, String> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let from_ms = params
        .get("fromMs")
        .and_then(Value::as_i64)
        .unwrap_or(now_ms - 24 * 3_600_000);
    let to_ms = params.get("toMs").and_then(Value::as_i64).unwrap_or(now_ms);
    let raw = crate::account::load_usage_history(from_ms, to_ms);
    let points: Vec<Value> = downsample_history(&raw, HISTORY_BUCKET_MS)
        .iter()
        .map(|p| {
            json!({
                "ts": p.ts,
                "fiveHour": p.five_hour,
                "sevenDay": p.seven_day,
                "sevenDaySonnet": p.seven_day_sonnet,
            })
        })
        .collect();
    Ok(Value::Array(points))
}

// Codex occupancy time series — the codex parallel of `serve_usage_history`.
// Pure disk read of the snapshots `codex_source` persists on every usage fetch
// (`codex_usage_history::record_snapshot`), so it stays on this thread too. The
// points are already camelCase-`Serialize` with 0–100 percentages (no ×100 on
// the wire, unlike Claude's 0–1 fractions), so we serialize them verbatim; the
// mobile chart does the /100 normalization the same way the desktop chart reads
// them raw. Window defaults to the last 24h.
fn serve_codex_usage_history(params: &Value) -> Result<Value, String> {
    let now_ms = chrono::Utc::now().timestamp_millis();
    let from_ms = params
        .get("fromMs")
        .and_then(Value::as_i64)
        .unwrap_or(now_ms - 24 * 3_600_000);
    let to_ms = params.get("toMs").and_then(Value::as_i64).unwrap_or(now_ms);
    let points = crate::codex_usage_history::load_codex_usage_history(from_ms, to_ms);
    serde_json::to_value(points).map_err(|e| e.to_string())
}

// Main-session invocations plus subagent sidecars, sorted by timestamp
// (mirrors `LocalBackend::get_skill_history` / `/skill_history`).
fn serve_skill_history(params: &Value) -> Result<Value, String> {
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
fn serve_guard_analyze(params: &Value) -> Result<Value, String> {
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
    let analysis = crate::llm_provider::complete_routed(
        &cfg,
        crate::llm_provider::ModelSlot::Fast,
        &prompt,
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
fn serve_session_search(params: &Value) -> Result<Value, String> {
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
fn serve_wiki_list(_params: &Value) -> Result<Value, String> {
    serde_json::to_value(crate::wiki::list_docs()).map_err(|e| e.to_string())
}

// One file — entry or asset — from a wiki doc version, base64-framed so
// the mobile client can decode markdown/html text or blob-serve assets
// (imgs/css/js) referenced by an htmlDir doc. `version` defaults to the
// current one. The 16 MiB per-file cap keeps a single frame bounded even
// though a whole doc can reach 100 MiB.
fn serve_wiki_file(params: &Value) -> Result<Value, String> {
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

// Full-text search over every published doc's metadata + entry body
// (mirrors the desktop's `search_wiki_docs`). Returns WikiSearchHit rows
// (slug / field=meta|content / snippet). The <2-char guard matches the
// session_search arm so a single keystroke doesn't scan every doc.
fn serve_wiki_search(params: &Value) -> Result<Value, String> {
    let query = params.get("query").and_then(Value::as_str).unwrap_or("");
    if query.trim().len() < 2 {
        return Ok(Value::Array(Vec::new()));
    }
    serde_json::to_value(crate::wiki::search_docs(query)).map_err(|e| e.to_string())
}

// Export one doc as a downloadable artifact (single file for
// markdown/html, a store-only zip of the whole version dir for htmlDir),
// base64-framed. 64 MiB cap — a doc version can hold up to 100 MiB, but
// a single export frame stays bounded.
fn serve_wiki_export(params: &Value) -> Result<Value, String> {
    use base64::Engine as _;
    let slug = params.get("slug").and_then(Value::as_str).ok_or("missing slug")?;
    let version =
        params.get("version").and_then(Value::as_str).unwrap_or("current");
    let export = crate::wiki::export_doc(slug, version)?;
    const MAX_WIKI_EXPORT_BYTES: usize = 64 * 1024 * 1024;
    if export.bytes.len() > MAX_WIKI_EXPORT_BYTES {
        return Err(format!("wiki export too large: {} bytes", export.bytes.len()));
    }
    Ok(json!({
        "filename": export.filename,
        "mime": export.mime,
        "base64": base64::engine::general_purpose::STANDARD.encode(&export.bytes),
    }))
}

// Absolute path of this host's pure-chat workspace, created on demand.
// The phone pins it in its new-session sheet; like the desktop it cannot
// derive the path itself (it's under the *desktop* host's home).
fn serve_chat_workspace(_params: &Value) -> Result<Value, String> {
    let path = crate::chat_workspace::ensure_chat_workspace()?;
    Ok(json!({ "path": path }))
}

// The monitored agent sources ({name, enabled, available}), so the mobile
// launcher can restrict the tool selector to the sources actually being watched
// — mirrors the desktop's `get_sources_config` Tauri command.
fn serve_sources_config(_params: &Value) -> Result<Value, String> {
    let sources = crate::agent_source::get_sources_config_local();
    serde_json::to_value(sources).map_err(|e| e.to_string())
}

// Directory picker for the new-session composer. Deliberately NOT gated
// on `known_workspaces` the way the explorer/git methods below are: the
// point is to reach a directory that has never had a session. It carries
// its own boundary instead (home + off-home known workspaces, canonical,
// directories only) — see `workspace_browse`.
fn serve_browse_dir(params: &Value) -> Result<Value, String> {
    let path = params.get("path").and_then(Value::as_str);
    let resp = crate::workspace_browse::browse_dir(path, &known_workspaces())?;
    serde_json::to_value(resp).map_err(|e| e.to_string())
}

// ── Write methods ────────────────────────────────────────────────
// All of these run inside `spawn_blocking` (see ws_connect_once), so
// process spawns / kills / file writes never block the ws runtime.
fn serve_spawn_session(params: &Value) -> Result<Value, String> {
    let req: crate::session_launch::SpawnSessionRequest =
        serde_json::from_value(params.clone())
            .map_err(|e| format!("bad spawn_session params: {e}"))?;
    // Thread the phone-provided session id through so a dropped reply
    // frame is still recoverable: the phone confirms success by finding
    // this exact id in a later `sessions` snapshot (relay delivery is
    // best-effort — see registry::forward). Codex mints its own thread id and
    // ignores the preassigned one, so a dropped reply for a codex spawn is not
    // recoverable by preassigned id — the phone must read the real id off the
    // reply frame.
    // Idempotent-resend guard: the phone resends this same req (same preassigned
    // session_id) when the reply frame was lost in transit. If we already spawned
    // this id, return the prior success instead of launching a duplicate process.
    let preassigned: Option<String> = req
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(id) = preassigned.as_deref() {
        if let Some(pid) = spawned_session_pid(id) {
            return serde_json::to_value(crate::session_launch::SpawnSessionResponse {
                pid,
                session_id: Some(id.to_string()),
            })
            .map_err(|e| e.to_string());
        }
    }
    let spec = crate::agent_source::SpawnSpec {
        workspace_path: req.workspace_path.clone(),
        prompt: req.prompt.clone(),
        model: req.model.clone(),
        effort: req.effort.clone(),
        permission_mode: req.permission_mode.clone(),
        session_id: req.session_id.clone(),
        entrypoint: String::new(),
    };
    let resp =
        crate::agent_source::spawn_session(req.tool.as_deref().unwrap_or("claude"), &spec)?;
    record_if_dedupable(preassigned.as_deref(), resp.session_id.as_deref(), resp.pid);
    serde_json::to_value(resp).map_err(|e| e.to_string())
}

fn serve_resume_session(params: &Value) -> Result<Value, String> {
    let req: crate::auto_resume::ResumeSessionRequest =
        serde_json::from_value(params.clone())
            .map_err(|e| format!("bad resume_session params: {e}"))?;
    // A "done" task resumed from mobile is active again — drop the done mark so
    // it re-surfaces as needs-review (next snapshot re-enriches user_mark).
    crate::session_mark::clear_done_on_resume(&req.session_id, &req.workspace_path);
    // Route by source (blank → claude); manual resume is untracked → no-op box.
    crate::agent_source::resume_session(
        &req.agent_source,
        &crate::agent_source::ResumeSpec {
            session_id: req.session_id.clone(),
            workspace_path: req.workspace_path.clone(),
            prompt: req.prompt.clone().unwrap_or_else(|| "continue".to_string()),
            model: req.model.clone(),
            effort: req.effort.clone(),
            permission_mode: req.permission_mode.clone(),
        },
        Box::new(|_| {}),
    )?;
    Ok(json!({ "ok": true }))
}

fn serve_enqueue_message(params: &Value) -> Result<Value, String> {
    let req: crate::pending_message::EnqueueMessageRequest =
        serde_json::from_value(params.clone())
            .map_err(|e| format!("bad enqueue_message params: {e}"))?;
    crate::pending_message::enqueue(&req.session_id, &req.workspace_path, &req.text)?;
    Ok(json!({ "ok": true }))
}

fn serve_cancel_pending_message(params: &Value) -> Result<Value, String> {
    let req: crate::pending_message::CancelMessageRequest = serde_json::from_value(params.clone())
        .map_err(|e| format!("bad cancel_pending_message params: {e}"))?;
    crate::pending_message::remove_at(&req.session_id, req.index)?;
    Ok(json!({ "ok": true }))
}

// pid 0 would signal the desktop's own process group — reject before
// it ever reaches kill() (same guard as the `/interrupt` endpoint).
fn serve_interrupt(params: &Value) -> Result<Value, String> {
    let pid = params.get("pid").and_then(Value::as_u64).unwrap_or(0) as u32;
    if pid == 0 {
        return Err("missing or invalid pid".into());
    }
    crate::session::interrupt_pid_impl(pid)?;
    Ok(json!({ "ok": true }))
}

fn serve_stop(params: &Value) -> Result<Value, String> {
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

fn serve_stop_workspace(params: &Value) -> Result<Value, String> {
    let path = params
        .get("workspacePath")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .ok_or("missing workspacePath")?;
    crate::session::kill_workspace_impl(path)?;
    Ok(json!({ "ok": true }))
}

fn serve_session_mark(params: &Value) -> Result<Value, String> {
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
fn serve_upload_attachment(params: &Value) -> Result<Value, String> {
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
fn serve_attachments_exist(params: &Value) -> Result<Value, String> {
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

fn serve_session_read(params: &Value) -> Result<Value, String> {
    let req: crate::session_read::MarkSessionsReadRequest =
        serde_json::from_value(params.clone())
            .map_err(|e| format!("bad session_read params: {e}"))?;
    crate::session_read::mark_read(&req.items)?;
    Ok(json!({ "ok": true }))
}

// ── Repository "仓库" surface ─────────────────────────────────────
// A loose-ends view over every git repo reachable from a known session
// workspace: worktrees with commits not merged back into main, and the
// main branch ahead of its upstream (unpushed). Access envelope matches
// the read-only file explorer — `known` is the same session-workspace
// list `/git_status` builds. push/pull run real `git` here (network),
// which is fine: this whole handler runs inside `spawn_blocking`.
fn serve_repo_list(_params: &Value) -> Result<Value, String> {
    let known = known_workspaces();
    serde_json::to_value(crate::git_ops::list_repos(&known)).map_err(|e| e.to_string())
}

fn serve_repo_detail(params: &Value) -> Result<Value, String> {
    let root = params.get("root").and_then(Value::as_str).ok_or("missing root")?;
    let known = known_workspaces();
    // Unwrap the core `Result` before serialising — `to_value` on the `Result`
    // itself emits serde's `{"Ok": {..}}` / `{"Err": ".."}` enum tagging, which
    // buries the payload one level down and crashes the mobile clients.
    let detail = crate::git_ops::repo_detail(root, &known)?;
    serde_json::to_value(detail).map_err(|e| e.to_string())
}

fn serve_repo_push(params: &Value) -> Result<Value, String> {
    let root = params.get("root").and_then(Value::as_str).ok_or("missing root")?;
    let known = known_workspaces();
    let res = crate::git_ops::repo_push(root, &known)?;
    serde_json::to_value(res).map_err(|e| e.to_string())
}

fn serve_repo_pull(params: &Value) -> Result<Value, String> {
    let root = params.get("root").and_then(Value::as_str).ok_or("missing root")?;
    let known = known_workspaces();
    let res = crate::git_ops::repo_pull(root, &known)?;
    serde_json::to_value(res).map_err(|e| e.to_string())
}

// ── Account & usage (mobile 「账号与用量」 page) ─────────────────────────────────

/// Bucket width for the mobile occupancy chart. Every usage fetch appends a
/// snapshot, so the raw series runs at a ~10s median cadence — a 24h window is
/// ~8k points (measured on Boss's machine: 7776), which is ~700KB of JSON over
/// the relay and an SVG path far finer than the phone has pixels. 5-min buckets
/// cap a 24h window at 288 points.
const HISTORY_BUCKET_MS: i64 = 5 * 60 * 1000;

/// Keep the last sample of each `bucket_ms`-wide bucket. Input is ascending by
/// `ts` (`load_usage_history` sorts); output stays ascending. "Last, not
/// average" on purpose: the chart shows occupancy at a point in time, and a
/// bucket's final reading is the one that actually held at that moment.
fn downsample_history(
    points: &[crate::account::UsageHistoryPoint],
    bucket_ms: i64,
) -> Vec<crate::account::UsageHistoryPoint> {
    let bucket_ms = bucket_ms.max(1);
    let mut out: Vec<crate::account::UsageHistoryPoint> = Vec::new();
    for p in points {
        let bucket = p.ts.div_euclid(bucket_ms);
        match out.last_mut() {
            Some(last) if last.ts.div_euclid(bucket_ms) == bucket => *last = p.clone(),
            _ => out.push(p.clone()),
        }
    }
    out
}

/// Flatten `AccountInfo` into the camelCase shape the mobile page consumes:
/// profile fields plus one bar per rate-limit window. `utilization` and
/// `prevUtilization` are the 0–1 fraction (`parse_usage` already divided the
/// API's 0–100), same convention the desktop UsagePanel renders.
fn claude_account_json(info: &crate::account::AccountInfo) -> Value {
    let bar = |label: &str, s: &crate::account::UsageStats| {
        json!({
            "label": label,
            "utilization": s.utilization,
            "resetsAt": s.resets_at,
            "prevUtilization": s.prev_utilization,
        })
    };
    let mut bars = Vec::new();
    if let Some(ref s) = info.five_hour {
        bars.push(bar("5h", s));
    }
    if let Some(ref s) = info.seven_day {
        bars.push(bar("7d Opus", s));
    }
    for sc in &info.seven_day_scoped {
        bars.push(json!({
            "label": format!("7d {}", sc.model_label),
            "utilization": sc.utilization,
            "resetsAt": sc.resets_at,
            "prevUtilization": sc.prev_utilization,
        }));
    }
    json!({
        "email": info.email,
        "fullName": info.full_name,
        "organizationName": info.organization_name,
        "plan": info.plan,
        "usageSource": info.usage_source,
        "bars": bars,
    })
}

/// Claude account + the other sources' usage bars, in one payload. A failed
/// Claude fetch (offline, not logged in) is reported as `claudeError` instead of
/// failing the whole request — the other sources and the page shell still render.
///
/// Must run on a thread with **no tokio runtime context**: the fetches below are
/// blocking wrappers that create their own runtime, and the relay's request
/// handler itself runs inside `spawn_blocking`.
fn account_usage_payload() -> Value {
    let (claude, claude_error) = match crate::account::fetch_account_info_blocking() {
        Ok(info) => (Some(claude_account_json(&info)), None),
        Err(e) => (None, Some(e)),
    };
    // Claude is handled above (its AccountInfo carries the previous-period
    // marker that `usage_summary` drops); the rest come through the same
    // normalised summary the tray menu uses.
    let sources: Vec<crate::backend::SourceUsageSummary> = crate::agent_source::build_sources()
        .iter()
        .filter(|s| s.api_name() != "claude" && s.is_available())
        .filter_map(|s| s.usage_summary())
        .collect();
    json!({ "claude": claude, "claudeError": claude_error, "sources": sources })
}

// ── WebSocket client (outbound connection + run loop) ───────────────────────

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
        *ENC_KEY.lock().unwrap() = None;
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

    // auth as agent. We send the HKDF-derived channel token, NOT the raw
    // pairing secret: the relay routes by a one-way function of the secret and
    // can no longer recover it (and thus can't derive `enc_key`). The AEAD key
    // is cached for `encode_payload`/`decode_inbound_payload` to seal/open with.
    let keys = crate::relay_crypto::derive_keys(&cfg.secret);
    *ENC_KEY.lock().unwrap() = Some(keys.enc_key);
    let auth =
        json!({ "type": "auth", "role": "agent", "secret": keys.channel_token }).to_string();
    sink.send(Message::Text(auth.into()))
        .await
        .map_err(|e| format!("auth send: {e}"))?;
    // Between here and the `authed` frame the socket is up but unroutable, and
    // pongs keep refreshing `last_inbound` — a relay that accepts the socket
    // but never auths would otherwise stall silently with no log line at all.
    crate::log_debug("[mobile-relay] socket open, auth sent");

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Outbound>();
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
                        reset_sessions_dedup();
                        crate::log_debug("[mobile-relay] connected");
                        // Reconnected with clients already on the channel: push
                        // the current state now instead of waiting for the next
                        // scan-driven change (which may be minutes away when
                        // every session is idle). No-op when no provider is set.
                        if frame.get("clients").and_then(Value::as_u64).unwrap_or(0) > 0 {
                            push_snapshot_on_connect();
                        }
                    }
                    Some("presence") => {
                        if let Some(n) = frame.get("clients").and_then(Value::as_u64) {
                            let prev = CLIENTS.swap(n as usize, Ordering::SeqCst);
                            // A client just joined: drop the dedup hash and push
                            // the current state to it right away, even when
                            // nothing changed for existing clients. Without this
                            // active push a phone connecting mid-idle would sit
                            // on a blank task list until some session file next
                            // changed (see push_snapshot_on_connect).
                            if n as usize > prev {
                                reset_sessions_dedup();
                                push_snapshot_on_connect();
                            }
                            // Everyone's gone: forget the device list immediately
                            // rather than waiting out the stale timeout.
                            if n == 0 {
                                clear_clients();
                            }
                        }
                    }
                    Some("msg") => {
                        // Every `msg` payload is a sealed `{enc:"box",…}` envelope;
                        // open it (and inflate if it was gzipped) before dispatch. A
                        // payload we can't open is dropped — there is no plaintext
                        // path under the hard cutover.
                        if let Some(payload) = frame.get("payload").and_then(decode_inbound_payload)
                        {
                            // Requests are handled one at a time (this arm awaits
                            // before the next frame is read), so queue wait is
                            // inferable from adjacent [relay-timing] lines: the
                            // previous line's end is the next request's start.
                            let method = payload
                                .get("method")
                                .and_then(Value::as_str)
                                .or_else(|| payload.get("event").and_then(Value::as_str))
                                .unwrap_or("?")
                                .to_string();
                            let started = std::time::Instant::now();
                            // Blocking file IO (write_response / pending scans)
                            // must not run on the ws runtime thread.
                            let reply = tokio::task::spawn_blocking(move || {
                                handle_client_payload(&payload)
                            })
                            .await
                            .ok()
                            .flatten();
                            let handle_ms = started.elapsed().as_millis();
                            if let Some(reply) = reply {
                                let out = encode_payload(&reply);
                                let Outbound::Text(ref wire) = out;
                                crate::log_debug(&format!(
                                    "[relay-timing] method={method} handle_ms={handle_ms} wire_bytes={}",
                                    wire.len()
                                ));
                                send_out(out);
                            } else if handle_ms >= 50 {
                                crate::log_debug(&format!(
                                    "[relay-timing] method={method} handle_ms={handle_ms} no-reply"
                                ));
                            }
                        } else {
                            // A dropped envelope is invisible to the phone (it just
                            // times out) — make version/key mismatches greppable.
                            crate::log_debug("[relay-timing] undecodable msg payload dropped");
                        }
                    }
                    Some("error") => {
                        return Err(format!("relay error: {frame}"));
                    }
                    _ => {}
                }
            }
            outbound = rx.recv() => {
                let Some(out) = outbound else { return Ok(()) };
                let Outbound::Text(s) = out;
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

    // ── spawn_session idempotent-resend dedup (方案 C 桌面去重) ──────────────

    #[test]
    fn dedup_unknown_id_is_none() {
        assert_eq!(spawned_session_pid("dedup-never-spawned-xyz"), None);
    }

    #[test]
    fn dedup_records_claude_but_not_codex() {
        // claude honors the phone-preassigned id (resp id == preassigned) → a
        // resend of the same id must find the prior pid and be deduped.
        record_if_dedupable(Some("dedup-claude-id"), Some("dedup-claude-id"), 111);
        assert_eq!(spawned_session_pid("dedup-claude-id"), Some(111));

        // codex mints its own thread id and ignores the preassigned one (resp id
        // != preassigned) → NOT dedupable, so a resend of the preassigned id must
        // not find a cached pid (it would return the wrong session otherwise).
        record_if_dedupable(Some("dedup-phone-preassigned"), Some("dedup-codex-real"), 222);
        assert_eq!(spawned_session_pid("dedup-phone-preassigned"), None);
    }

    // ── 方案 A early submit-ack ──────────────────────────────────────────────

    #[test]
    fn ackable_covers_writes_not_reads() {
        for m in [
            "spawn_session",
            "resume_session",
            "enqueue_message",
            "cancel_pending_message",
            "interrupt",
            "stop",
            "stop_workspace",
            "session_mark",
            "upload_attachment",
            "decision_answer",
        ] {
            assert!(is_ackable_method(m), "{m} should be ackable");
        }
        for m in ["pending_snapshot", "tail", "tail_delta", "wiki_list", ""] {
            assert!(!is_ackable_method(m), "{m} should not be ackable");
        }
    }

    #[test]
    fn sources_config_returns_source_list() {
        // The mobile launcher restricts its tool selector to these entries, so
        // the method must return an array of {name, enabled, available}.
        let v = serve_request("sources_config", &serde_json::json!({})).unwrap();
        let arr = v.as_array().expect("sources_config returns an array");
        assert!(
            arr.iter().any(|s| s["name"] == serde_json::json!("codex")),
            "codex source must be present in the config: {v}"
        );
        for s in arr {
            assert!(s["name"].is_string(), "each entry has a name: {s}");
            assert!(s["enabled"].is_boolean(), "each entry has enabled: {s}");
            assert!(s["available"].is_boolean(), "each entry has available: {s}");
        }
    }

    #[test]
    fn ack_frame_carries_event_and_req_id() {
        let f = ack_frame(&json!("abc-7"));
        assert_eq!(f["event"], "ack");
        assert_eq!(f["req_id"], "abc-7");
    }

    // ── decision_answer idempotent-resend dedup (弱网答复重传去重) ────────────

    #[test]
    fn decision_answer_dedup_primitive_records_and_isolates() {
        // Before recording, an id is not a dedup hit; after recording it is, and an
        // unrelated id is unaffected. This is the primitive the resend safety rests
        // on — a no-op stub (record does nothing) fails the middle assertion.
        let id = "dedup-decision-primitive-1";
        assert!(!decision_already_answered(id));
        record_decision_answered(id);
        assert!(decision_already_answered(id));
        assert!(!decision_already_answered("dedup-decision-primitive-unrelated"));
    }

    // ── resume_session idempotent-resend dedup (弱网重发双跑去重) ──────────────

    #[test]
    fn resume_dedup_primitive_records_and_isolates() {
        // A `resume_session` is an ackable write; on a lost ack the phone resends
        // it, and without dedup the second resend launches a *second* `claude`
        // turn on the same session (the two-roots double-submit we diagnosed).
        // Before recording, a session is not a dedup hit; after recording it is,
        // and an unrelated session is unaffected. The P1 no-op stub fails the
        // middle assertion (it always reports "not recently dispatched").
        let id = "dedup-resume-primitive-1";
        assert!(!resume_recently_dispatched(id));
        record_resume_dispatched(id);
        assert!(
            resume_recently_dispatched(id),
            "a resume dispatched this window must be a dedup hit"
        );
        assert!(!resume_recently_dispatched("dedup-resume-primitive-unrelated"));
    }

    #[test]
    fn decision_answer_resend_short_circuits_to_ok() {
        // The phone resends the answer req when a reply frame is lost. Once the
        // first answer was delivered (recorded), the resend must short-circuit to
        // Ok — NOT fall through to deliver → write_response's "no pending request"
        // Err (which a no-op dedup stub would surface, failing this test).
        let id = "dedup-decision-resend-2";
        record_decision_answered(id);
        let params = json!({ "kind": "elicitation", "id": id, "answers": {} });
        assert!(
            serve_decision_answer(&params).is_ok(),
            "a resend of an already-delivered answer must be an idempotent Ok"
        );
    }

    #[test]
    fn dedup_ignores_blank_preassigned() {
        record_if_dedupable(Some("   "), Some("whatever"), 333);
        record_if_dedupable(None, Some("whatever"), 333);
        // Nothing recorded under a blank/None key — spawned_session_pid("") stays None.
        assert_eq!(spawned_session_pid(""), None);
    }

    #[test]
    fn tail_delta_stops_offset_at_last_newline() {
        // Watcher/poll fires mid-flush: "A\n" is complete, record 2 is partial.
        // newOffset must stop at the newline (8), not jump to EOF, or the client
        // resumes past the half-written record and loses it forever.
        let tmp = std::env::temp_dir().join(format!(
            "fleet-tail-delta-{}-{}.jsonl",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::write(&tmp, b"{\"i\":1}\n{\"i\":2,\"partia").unwrap();
        let p = tmp.to_str().unwrap();

        let out = serve_tail_delta(&json!({ "path": p, "offset": 0 })).unwrap();
        assert_eq!(out["lines"].as_array().unwrap().len(), 1, "only complete line");
        let new_offset = out["newOffset"].as_u64().unwrap();
        assert_eq!(new_offset, 8, "offset stops at last newline, not EOF");

        // Rest of record 2 plus record 3 land; resuming from new_offset recovers both.
        fs::write(&tmp, b"{\"i\":1}\n{\"i\":2,\"partial\":true}\n{\"i\":3}\n").unwrap();
        let out2 = serve_tail_delta(&json!({ "path": p, "offset": new_offset })).unwrap();
        assert_eq!(
            out2["lines"].as_array().unwrap().len(),
            2,
            "record 2 recovered and record 3 read"
        );
        fs::remove_file(&tmp).ok();
    }

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

    /// Park a fleet__ask card for a session whose transcript says Fleet launched
    /// it, and point the CLI resolution at a recorder — so a resume, if one
    /// happens, is observable and lands on a harmless binary instead of a real
    /// `claude`.
    fn park_a_card(home: &std::path::Path, id: &str, resume_log: &std::path::Path) {
        let workspace = home.join("ws");
        let projects = home.join(".claude").join("projects").join("p");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&projects).unwrap();
        fs::write(
            projects.join("sess-mob.jsonl"),
            format!(
                "{{\"type\":\"user\",\"entrypoint\":\"{}\",\"cwd\":\"{}\"}}\n",
                crate::session_launch::NEW_SESSION_ENTRYPOINT,
                workspace.display()
            ),
        )
        .unwrap();

        let recorder = home.join("claude-recorder");
        fs::write(&recorder, format!("#!/bin/sh\necho resumed >> {}\n", resume_log.display())).unwrap();
        #[cfg(unix)]
        {
            let mut perms = fs::metadata(&recorder).unwrap().permissions();
            std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
            fs::set_permissions(&recorder, perms).unwrap();
        }
        fs::create_dir_all(home.join(".fleet")).unwrap();
        fs::write(
            home.join(".fleet").join("claude-binary.json"),
            serde_json::json!({ "override_path": recorder.to_string_lossy() }).to_string(),
        )
        .unwrap();

        let req = crate::mcp_ipc::FleetAskRequest {
            id: id.to_string(),
            session_id: "sess-mob".into(),
            workspace_name: "ws".into(),
            ai_title: None,
            timestamp: "2026-07-14T00:00:00Z".into(),
            parked: false,
            review_docs: vec![],
            questions: vec![crate::mcp_ipc::FleetAskQuestion {
                question: "保留兼容？".into(),
                header: "兼容".into(),
                multi_select: false,
                options: vec![],
                html: None,
                form_fields: vec![],
                images: vec![],
            }],
        };
        crate::parked::park(
            id,
            crate::parked::ParkedKind::FleetAsk,
            "sess-mob",
            workspace.to_str().unwrap(),
            &req,
        )
        .unwrap();
    }

    /// Answering a parked card **from the phone** has to do what answering it on
    /// the desktop does: wake the session up. The mobile relay used to write the
    /// response straight into the channel's `<id>.response.json` — but a parked
    /// card has no producer left polling that file, so the answer would land in
    /// an orphan file, the session would never resume, and the card would sit on
    /// the phone forever looking answered-but-stuck.
    #[test]
    fn a_phone_answer_to_a_parked_card_resumes_the_session() {
        let _guard = fleet_home_lock();
        let home = std::env::temp_dir().join(format!(
            "fleet-mobile-parked-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&home).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by fleet_home_lock.
        unsafe { std::env::set_var("FLEET_HOME", &home) };

        let resume_log = home.join("resumed.txt");
        park_a_card(&home, "card-mob", &resume_log);

        handle_answer(&serde_json::json!({
            "kind": "fleet-ask",
            "id": "card-mob",
            "answers": { "保留兼容？": "保留" },
            "cancelled": false
        }))
        .unwrap();

        // The session was woken up...
        let resumed = (0..50).any(|_| {
            if resume_log.is_file() {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
            false
        });
        assert!(resumed, "a phone answer must resume the parked session");

        // ...the card is resolved...
        assert!(
            !crate::parked::is_parked("card-mob"),
            "an answered card must not stay parked, or it would resume twice"
        );

        // ...and no orphan response file was left behind for nobody to read.
        let orphan = home
            .join(".fleet")
            .join("fleet-ask")
            .join("card-mob.response.json");
        assert!(
            !orphan.is_file(),
            "the answer must not be filed for a producer that no longer exists"
        );

        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&home);
    }

    /// The phone's snapshot *replaces* its card list (see `refreshPending` in
    /// mobile-web's App.tsx), so a card missing from it is a card that vanishes
    /// from the phone on the next reconcile. A parked card is still pending —
    /// it just lives in the parked store — and must show up here, flagged, or
    /// the timeout would still make the question disappear, only on mobile.
    #[test]
    fn the_phone_snapshot_carries_parked_cards_flagged() {
        let _guard = fleet_home_lock();
        let home = std::env::temp_dir().join(format!(
            "fleet-mobile-snap-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        fs::create_dir_all(&home).unwrap();
        let prev = std::env::var_os("FLEET_HOME");
        // SAFETY: serialised by fleet_home_lock.
        unsafe { std::env::set_var("FLEET_HOME", &home) };

        park_a_card(&home, "card-snap", &home.join("unused.txt"));
        let snap = serve_request("pending_snapshot", &serde_json::json!({})).unwrap();

        let asks = snap["fleetAsk"].as_array().expect("fleetAsk list");
        assert_eq!(asks.len(), 1, "the parked card must still be pending: {snap}");
        assert_eq!(asks[0]["id"], serde_json::json!("card-snap"));
        assert_eq!(
            asks[0]["parked"],
            serde_json::json!(true),
            "the phone needs the flag to badge the card as paused"
        );
        assert_eq!(
            asks[0]["questions"][0]["question"],
            serde_json::json!("保留兼容？"),
            "the question itself has to survive the trip"
        );

        unsafe {
            match prev {
                Some(p) => std::env::set_var("FLEET_HOME", p),
                None => std::env::remove_var("FLEET_HOME"),
            }
        }
        let _ = fs::remove_dir_all(&home);
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
            pairing_url(&cfg, None),
            format!("https://relay.example.com/#k={}", "s".repeat(64))
        );
        // A valid lang rides in the fragment after the secret.
        assert_eq!(
            pairing_url(&cfg, Some("zh")),
            format!("https://relay.example.com/#k={}&lang=zh", "s".repeat(64))
        );
        // Unknown languages are dropped rather than trusted verbatim.
        assert_eq!(
            pairing_url(&cfg, Some("fr")),
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

    /// Seed a live pending request file so `write_response`'s existence
    /// precheck passes — mirrors a real decision whose producer is still
    /// waiting. Content is irrelevant (the precheck only tests existence).
    fn seed_request(dir: &std::path::Path, id: &str) {
        fs::write(dir.join(format!("{id}.json")), "{}").unwrap();
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
            seed_request(&dir, "g1");
            seed_request(&dir, "g2");
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
    fn answer_unknown_id_writes_no_orphan_response() {
        // Regression: answering an id with no live pending request must not
        // write an orphan response file (and the HTTP layer maps the resulting
        // "no pending request" error to 404, not a raw 500). See the
        // `write_response` existence precheck across the six decision channels.
        with_temp_home(|| {
            let dir = fleet_subdir("guard");
            // Deliberately NOT seeded — no request file for "ghost".
            handle_client_payload(&json!({
                "event": "answer", "kind": "guard", "id": "ghost", "allow": true
            }));
            assert!(
                !dir.join("ghost.response.json").exists(),
                "responding to an unknown id must not write an orphan response file"
            );
        });
    }

    #[test]
    fn answer_elicitation_writes_response_file() {
        with_temp_home(|| {
            let dir = fleet_subdir("elicitation");
            seed_request(&dir, "e1");
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
            seed_request(&dir, "f1");
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
            seed_request(&dir, "p1");
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
            seed_request(&dir, "pp1");
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
            seed_request(&dir, "a1");
            seed_request(&dir, "a2");
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

    /// A record carrying an embedded screenshot and a whole file's contents —
    /// the shape that made one real session's `tail` reply 2.4 MB (1.7 MB even
    /// after gzip, because base64 doesn't compress). The mobile client renders
    /// none of it: `toolUseResult` isn't in its `RawMessage` at all, and an
    /// `image` block renders as the literal string "[图片]".
    fn fat_record() -> Value {
        let blob = "A".repeat(4096); // stands in for base64 image data
        json!({
            "type": "user",
            "uuid": "u-fat",
            "timestamp": "2026-07-13T00:00:00Z",
            "parentUuid": "u-prev",
            "sessionId": "s-1",
            "cwd": "/some/very/long/workspace/path",
            "entrypoint": "cli",
            "message": {
                "role": "user",
                "content": [
                    {"type": "image", "source": {"type": "base64", "media_type": "image/png",
                                                 "data": blob.clone()}},
                    {"type": "text", "text": "look at this"},
                    {"type": "tool_use", "id": "tu1", "name": "Read",
                     "input": {"file_path": "/a/b.rs", "offset": 1, "limit": 2000,
                               "junk": blob.clone()}}
                ],
                "usage": {"input_tokens": 1, "output_tokens": 2}
            },
            "toolUseResult": { "file": { "content": blob.clone() }, "stdout": blob }
        })
    }

    /// The `tail` reply must carry only what the mobile `RawMessage` declares.
    /// Shipping the raw records means the phone downloads megabytes of base64
    /// it decodes and then throws away — the "task detail takes forever / times
    /// out" bug (mobile `tail` runs on the default 15s timeout).
    #[test]
    fn request_tail_slims_payload_to_rendered_fields() {
        with_temp_home(|| {
            let path = write_jsonl("fat.jsonl", &[fat_record()]);
            let data = request_ok("tail", json!({"path": path, "n": 10}));
            let msgs = data.as_array().expect("array");
            assert_eq!(msgs.len(), 1);
            let m = &msgs[0];

            // Dropped: never referenced by the mobile client.
            assert!(m.get("toolUseResult").is_none(), "toolUseResult must be stripped");
            assert!(m.get("cwd").is_none(), "cwd must be stripped");
            assert!(m.get("sessionId").is_none(), "sessionId must be stripped");
            assert!(m.get("parentUuid").is_none(), "parentUuid must be stripped");
            // usage survives trimmed to the two rendered counters (per-turn line).
            assert_eq!(m["message"]["usage"], json!({"input_tokens": 1, "output_tokens": 2}));

            // Kept: the RawMessage field set the UI actually reads.
            assert_eq!(m["type"], "user");
            assert_eq!(m["uuid"], "u-fat", "uuid drives appendUnique dedup");
            assert_eq!(m["timestamp"], "2026-07-13T00:00:00Z");
            assert_eq!(m["message"]["role"], "user");

            let blocks = m["message"]["content"].as_array().expect("content blocks");
            assert_eq!(blocks.len(), 3, "block count preserved");
            // image: type kept (renders as "[图片]"), base64 payload dropped
            assert_eq!(blocks[0]["type"], "image");
            assert!(blocks[0].get("source").is_none(), "image base64 must be stripped");
            // text: kept verbatim
            assert_eq!(blocks[1]["text"], "look at this");
            // tool_use: only the fields the tool chip shows
            assert_eq!(blocks[2]["name"], "Read");
            assert_eq!(blocks[2]["input"]["file_path"], "/a/b.rs");
            assert!(blocks[2]["input"].get("junk").is_none(), "unused input must be stripped");

            // The whole point: the fat record collapses to a fraction of its size.
            let raw = fat_record().to_string().len();
            let slim = m.to_string().len();
            assert!(slim * 20 < raw, "slim ({slim}B) must be far smaller than raw ({raw}B)");
        });
    }

    /// `tail_delta` feeds the same `RawMessage` list (appendUnique by uuid), so
    /// it must slim identically — otherwise one new fat record re-inflates the
    /// transcript the initial `tail` just slimmed.
    #[test]
    fn request_tail_delta_slims_payload_too() {
        with_temp_home(|| {
            let path = write_jsonl("fat2.jsonl", &[fat_record()]);
            let data = request_ok("tail_delta", json!({"path": path, "offset": 0}));
            let lines = data["lines"].as_array().expect("lines");
            assert_eq!(lines.len(), 1);
            assert!(lines[0].get("toolUseResult").is_none(), "delta must strip toolUseResult");
            assert!(
                lines[0]["message"]["content"][0].get("source").is_none(),
                "delta must strip image base64"
            );
            assert_eq!(lines[0]["uuid"], "u-fat", "uuid preserved for dedup");
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
    fn request_wiki_search_matches_body_and_guards_short_query() {
        with_temp_home(|| {
            let src = crate::session::real_home_dir().unwrap().join("wiki-src");
            fs::create_dir_all(&src).unwrap();
            let file = src.join("note.md");
            fs::write(&file, "# 标题\n\n独特关键词 xyzzy 在正文里").unwrap();
            let doc =
                crate::wiki::publish(&file, None, None, std::path::Path::new("/ws/demo")).unwrap();

            // A body-only term (not in title/slug) is found with a content snippet.
            let data = request_ok("wiki_search", json!({"query": "xyzzy"}));
            let hits = data.as_array().expect("array");
            assert_eq!(hits.len(), 1);
            assert_eq!(hits[0]["slug"], doc.slug);
            assert_eq!(hits[0]["field"], "content");
            assert!(hits[0]["snippet"].as_str().unwrap().contains("xyzzy"));

            // <2-char queries short-circuit to an empty list (no scan).
            let data = request_ok("wiki_search", json!({"query": "x"}));
            assert_eq!(data.as_array().unwrap().len(), 0);
        });
    }

    #[test]
    fn request_wiki_export_frames_markdown_and_zips_htmldir() {
        with_temp_home(|| {
            // markdown doc → single-file export, filename from the slug basename.
            let src = crate::session::real_home_dir().unwrap().join("wiki-src");
            fs::create_dir_all(&src).unwrap();
            let file = src.join("guide.md");
            fs::write(&file, "# 指南\n\n正文").unwrap();
            let md = crate::wiki::publish(&file, None, None, std::path::Path::new("/ws/demo"))
                .unwrap();
            let data = request_ok("wiki_export", json!({"slug": md.slug}));
            assert_eq!(data["filename"], "guide.md");
            assert!(data["mime"].as_str().unwrap().contains("markdown"));
            assert!(String::from_utf8(decode_b64(&data)).unwrap().contains("正文"));

            // htmlDir doc → a zip (PK\x03\x04 magic), filename ends in .zip.
            let dir = crate::session::real_home_dir().unwrap().join("site");
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("index.html"), "<h1>hi</h1>").unwrap();
            let hd = crate::wiki::publish(&dir, None, None, std::path::Path::new("/ws/demo"))
                .unwrap();
            let data = request_ok("wiki_export", json!({"slug": hd.slug}));
            assert!(data["filename"].as_str().unwrap().ends_with(".zip"));
            assert_eq!(data["mime"], "application/zip");
            let bytes = decode_b64(&data);
            assert_eq!(&bytes[0..4], b"PK\x03\x04", "zip magic");
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

    /// The phone filters chat sessions out of its tasks list by comparing each
    /// session's `workspacePath` against this reply, so it must be the very same
    /// path the scanner stamps on a chat session — not merely "some path".
    #[test]
    fn request_chat_workspace_returns_the_scanner_path() {
        with_temp_home(|| {
            let data = request_ok("chat_workspace", json!({}));
            let path = data["path"].as_str().expect("reply must carry a path");
            assert!(
                crate::chat_workspace::is_chat_workspace(path),
                "relay handed back a path the scanner won't recognise as chat: {path}"
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
            seed_request(&dir, "g10");
            seed_request(&dir, "g11");
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
        use crate::session_launch::NEW_SESSION_ENTRYPOINT;
        let sessions = json!([
            {
                "id": "old", "isSubagent": false, "lastActivityMs": 100,
                "workspaceName": "w", "tokenSpeed": 42.0, "rateLimit": {"until": 1},
                "lastMessagePreview": "短预览", "userMark": "done", "aiTitle": null,
                "titleOverride": "手动重命名",
                "entrypoint": NEW_SESSION_ENTRYPOINT
            },
            {
                "id": "sub", "isSubagent": true, "lastActivityMs": 300,
                "workspaceName": "w", "entrypoint": NEW_SESSION_ENTRYPOINT
            },
            {
                "id": "ext", "isSubagent": false, "lastActivityMs": 250,
                "workspaceName": "w", "entrypoint": "vscode"
            },
            {
                "id": "new", "isSubagent": false, "lastActivityMs": 200,
                "workspaceName": "w",
                "lastMessagePreview": "长".repeat(500),
                "entrypoint": NEW_SESSION_ENTRYPOINT
            },
        ]);
        let slim = slim_sessions_snapshot(&sessions);
        let list = slim.as_array().expect("array");
        // Subagent + non-Fleet dropped; remainder sorted most-recent first.
        assert_eq!(list.len(), 2);
        assert_eq!(list[0]["id"], "new");
        assert_eq!(list[1]["id"], "old");
        // Desktop-only fields are gone, whitelisted ones survive, nulls dropped.
        assert!(list[1].get("tokenSpeed").is_none());
        assert!(list[1].get("rateLimit").is_none());
        assert!(list[1].get("aiTitle").is_none());
        // A human/agent title override must survive slimming — for Codex
        // sessions (no local ai_title) it's the *only* real title the phone
        // can show. Regression guard for the mobile "override title missing"
        // bug where titleOverride was absent from SNAPSHOT_FIELDS.
        assert_eq!(list[1]["titleOverride"], "手动重命名");
        assert_eq!(list[1]["userMark"], "done");
        assert_eq!(list[1]["lastMessagePreview"], "短预览");
        // Long previews are truncated with an ellipsis.
        let preview = list[0]["lastMessagePreview"].as_str().unwrap();
        assert_eq!(preview.chars().count(), SNAPSHOT_PREVIEW_CHARS + 1);
        assert!(preview.ends_with('…'));
    }

    #[test]
    fn slim_snapshot_keeps_subagent_of_kept_parent() {
        use crate::session_launch::NEW_SESSION_ENTRYPOINT;
        let sessions = json!([
            {
                "id": "main", "isSubagent": false, "lastActivityMs": 100,
                "workspaceName": "w", "entrypoint": NEW_SESSION_ENTRYPOINT
            },
            // Subagent of a kept main → survives, carries the drill-down fields.
            {
                "id": "agent-abc", "isSubagent": true, "parentSessionId": "main",
                "agentType": "Explore", "runningSubagentCount": 0,
                "lastActivityMs": 90, "jsonlPath": "/p/subagents/agent-abc.jsonl",
                "workspaceName": "w", "entrypoint": NEW_SESSION_ENTRYPOINT
            },
            // Subagent whose parent is absent → dropped (no orphan rows).
            {
                "id": "agent-orphan", "isSubagent": true, "parentSessionId": "gone",
                "lastActivityMs": 95, "workspaceName": "w",
                "entrypoint": NEW_SESSION_ENTRYPOINT
            },
        ]);
        let slim = slim_sessions_snapshot(&sessions);
        let list = slim.as_array().expect("array");
        let ids: Vec<&str> = list.iter().filter_map(|s| s["id"].as_str()).collect();
        assert!(ids.contains(&"main"));
        assert!(ids.contains(&"agent-abc"), "subagent of kept parent survives");
        assert!(!ids.contains(&"agent-orphan"), "orphan subagent dropped");
        let sub = list.iter().find(|s| s["id"] == "agent-abc").unwrap();
        assert_eq!(sub["parentSessionId"], "main");
        assert_eq!(sub["agentType"], "Explore");
        assert_eq!(sub["jsonlPath"], "/p/subagents/agent-abc.jsonl");
    }

    #[test]
    fn tool_result_digest_carries_agent_id() {
        // The Agent toolUseResult shape: status + the subagent's agentId, which
        // the phone's "打开子代理" button needs to look the subagent up.
        let meta = json!({
            "status": "completed",
            "agentId": "abc-123",
            "totalToolUseCount": 7
        });
        let d = tool_result_digest(&meta).expect("digest");
        assert_eq!(d["agentStatus"], "completed");
        assert_eq!(d["agentId"], "abc-123");
        assert_eq!(d["toolUses"], 7);
    }

    #[test]
    fn slim_snapshot_caps_session_count() {
        use crate::session_launch::NEW_SESSION_ENTRYPOINT;
        let total = SNAPSHOT_MAX_SESSIONS + 100;
        let many: Vec<Value> = (0..total)
            .map(|i| {
                json!({
                    "id": format!("s{i}"), "isSubagent": false, "lastActivityMs": i,
                    "entrypoint": NEW_SESSION_ENTRYPOINT
                })
            })
            .collect();
        let slim = slim_sessions_snapshot(&Value::Array(many));
        let list = slim.as_array().unwrap();
        assert_eq!(list.len(), SNAPSHOT_MAX_SESSIONS);
        // Kept the MOST recent ones (highest lastActivityMs).
        assert_eq!(list[0]["id"], format!("s{}", total - 1));
        assert_eq!(list.last().unwrap()["id"], format!("s{}", total - SNAPSHOT_MAX_SESSIONS));
    }

    #[test]
    fn slim_snapshot_drops_non_fleet_and_keeps_all_fleet() {
        use crate::session_launch::NEW_SESSION_ENTRYPOINT;
        // 130 Fleet-owned sessions (> the old 120 cap, < the new 500 cap) plus
        // 10 non-Fleet sessions given the MOST-recent activity so that, under the
        // buggy filter-after-truncate order, they would steal the top slots.
        let mut sessions: Vec<Value> = (0..130)
            .map(|i| {
                json!({
                    "id": format!("fleet{i}"),
                    "isSubagent": false,
                    "lastActivityMs": i,
                    "entrypoint": NEW_SESSION_ENTRYPOINT,
                })
            })
            .collect();
        sessions.extend((0..10).map(|i| {
            json!({
                "id": format!("ext{i}"),
                "isSubagent": false,
                "lastActivityMs": 100_000 + i, // most recent
                "entrypoint": "vscode",
            })
        }));
        let slim = slim_sessions_snapshot(&Value::Array(sessions));
        let list = slim.as_array().expect("array");
        // Every Fleet-owned session survives (cap raised above 130) and no
        // non-Fleet session is present (filter applied before truncation).
        assert_eq!(list.len(), 130);
        assert!(
            list.iter().all(|s| s["id"].as_str().unwrap().starts_with("fleet")),
            "non-Fleet sessions must not appear in the mobile snapshot"
        );
    }

    /// Encode `buf` as a PNG — the fixture format decision assets arrive in.
    fn png_of(buf: image::RgbImage) -> Vec<u8> {
        let mut png = Vec::new();
        image::DynamicImage::ImageRgb8(buf)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        png
    }

    #[test]
    fn downscale_hits_target_for_typical_image() {
        // A large but smoothly-varying (highly compressible) image — the common
        // case for a screenshot or chart. JPEG should reach the ≤50 KB target.
        let (w, h) = (2000u32, 1500u32);
        let mut buf = image::RgbImage::new(w, h);
        for (x, y, px) in buf.enumerate_pixels_mut() {
            *px = image::Rgb([(x / 16) as u8, (y / 16) as u8, ((x + y) / 32) as u8]);
        }
        let png = png_of(buf);

        let (out, mime) = downscale_decision_asset(png, "image/png");
        assert_eq!(mime, "image/jpeg");
        assert!(
            out.len() <= DECISION_ASSET_TARGET_BYTES,
            "typical image must reach the target, got {}",
            out.len()
        );
        image::load_from_memory(&out).expect("output must decode");
    }

    #[test]
    fn downscale_stays_under_hard_cap_for_incompressible_noise() {
        // A high-frequency (poorly compressible) pattern is the worst case: the
        // quality ladder can't reach the target at full res, so the loop shrinks
        // resolution. The result must still land under the hard cap.
        let (w, h) = (2000u32, 1500u32);
        let mut buf = image::RgbImage::new(w, h);
        for (x, y, px) in buf.enumerate_pixels_mut() {
            *px = image::Rgb([
                (x ^ y) as u8,
                (x.wrapping_mul(31) ^ y) as u8,
                (x.wrapping_add(y)) as u8,
            ]);
        }
        let png = png_of(buf);

        let (out, mime) = downscale_decision_asset(png.clone(), "image/png");
        assert_eq!(mime, "image/jpeg");
        assert!(out.len() < png.len(), "must shrink vs the source PNG");
        assert!(
            out.len() <= DECISION_ASSET_HARD_CAP_BYTES,
            "even incompressible noise must stay under the hard cap, got {}",
            out.len()
        );
        image::load_from_memory(&out).expect("output must decode");
    }

    #[test]
    fn downscale_reencodes_small_image_keeping_resolution() {
        // "Compress every image" — a small image already under the target is
        // still re-encoded to JPEG, but at its original resolution (no upscaling,
        // no needless shrinking).
        let mut buf = image::RgbImage::new(100, 80);
        for (x, y, px) in buf.enumerate_pixels_mut() {
            *px = image::Rgb([(x * 2) as u8, (y * 2) as u8, 128]);
        }
        let png = png_of(buf);

        let (out, mime) = downscale_decision_asset(png, "image/png");
        assert_eq!(mime, "image/jpeg");
        assert!(out.len() <= DECISION_ASSET_TARGET_BYTES);
        let decoded = image::load_from_memory(&out).expect("output must decode");
        assert_eq!((decoded.width(), decoded.height()), (100, 80));
    }

    #[test]
    fn downscale_passes_through_undecodable_bytes() {
        // A non-image payload (e.g. an SVG) can't be re-encoded — it is returned
        // untouched with its original mime so the caller's frame guard applies.
        let svg = b"<svg xmlns='http://www.w3.org/2000/svg'></svg>".to_vec();
        let (out, mime) = downscale_decision_asset(svg.clone(), "image/svg+xml");
        assert_eq!(out, svg);
        assert_eq!(mime, "image/svg+xml");
    }

    #[test]
    fn downscale_passes_truncated_image_through_untouched() {
        // A PNG magic header followed by garbage can't be decoded, so it is
        // returned byte-for-byte with its original mime rather than mangled.
        let broken = vec![0x89, b'P', b'N', b'G', 1, 2, 3, 4];
        let (out, mime) = downscale_decision_asset(broken.clone(), "image/png");
        assert_eq!(out, broken);
        assert_eq!(mime, "image/png");
    }

    #[test]
    fn snapshot_hash_dedups_identical_content() {
        use crate::session_launch::NEW_SESSION_ENTRYPOINT;
        let ep = NEW_SESSION_ENTRYPOINT;
        let a = slim_sessions_snapshot(&json!([{"id": "x", "lastActivityMs": 1, "entrypoint": ep}]));
        let b = slim_sessions_snapshot(&json!([{"id": "x", "lastActivityMs": 1, "entrypoint": ep}]));
        let c = slim_sessions_snapshot(&json!([{"id": "x", "lastActivityMs": 2, "entrypoint": ep}]));
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

        // appCommit from client_hello is surfaced per device; an empty or
        // "unknown" commit (a bundle built without a commit source, or the
        // Harmony client which omits the field) normalizes to None so the
        // desktop shows no version and raises no stale flag for it.
        handle_client_payload(&json!({
            "event": "client_hello", "clientId": "ver", "label": "iPhone",
            "platform": "ios", "pushSubscribed": false, "appCommit": "ea6c003",
        }));
        handle_client_payload(&json!({
            "event": "client_hello", "clientId": "nover", "label": "unknown-build",
            "platform": "ios", "pushSubscribed": false, "appCommit": "unknown",
        }));
        handle_client_payload(&hello("absent", "legacy", false)); // no appCommit key
        {
            let devs = live_devices();
            let commit = |id: &str| devs.iter().find(|d| d.client_id == id).unwrap().app_commit.clone();
            assert_eq!(commit("ver"), Some("ea6c003".to_string()), "real commit parsed");
            assert_eq!(commit("nover"), None, "\"unknown\" normalized to None");
            assert_eq!(commit("absent"), None, "missing appCommit → None");
        }
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

        // gzip compression gate — folded in here because it shares the global
        // CLIENTS_REGISTRY and would race a separate #[test].
        assert!(!all_clients_support_gzip(), "empty registry → never compress");
        handle_client_payload(&hello_gzip("g1", true));
        assert!(all_clients_support_gzip(), "sole gzip-capable client → compress");
        // A client that can't inflate (omits the flag → default false) vetoes
        // compression for the whole channel: the relay broadcasts one frame to
        // everyone, so we can't gzip for some clients only.
        handle_client_payload(&hello("g2", "old PWA", false));
        assert!(!all_clients_support_gzip(), "one non-gzip client forces uncompressed");
        clear_clients();
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

    /// Open a sealed outbound frame the way a paired phone would: parse the
    /// `{type:msg,payload}` envelope and decrypt (and inflate) the payload. Used
    /// by tests that assert on what a phone actually receives.
    fn decode_out(text: &str) -> Value {
        let frame: Value = serde_json::from_str(text).expect("valid frame");
        assert_eq!(frame["type"], "msg", "outbound business frames are msg frames");
        decode_inbound_payload(&frame["payload"]).expect("sealed payload opens")
    }

    fn hello_gzip(client_id: &str, supports_gzip: bool) -> Value {
        json!({
            "event": "client_hello",
            "clientId": client_id,
            "label": "x",
            "platform": "ios",
            "pushSubscribed": false,
            "supportsGzip": supports_gzip,
        })
    }

    /// Only the windows the API actually returned become bars, and the 0–1
    /// fraction is passed through untouched (the mobile page multiplies by 100).
    #[test]
    fn claude_account_json_emits_one_bar_per_present_window() {
        use crate::account::{AccountInfo, ScopedUsage, UsageStats};
        let info = AccountInfo {
            email: "boss@example.com".into(),
            plan: "Claude Max".into(),
            usage_source: "anthropic".into(),
            five_hour: Some(UsageStats {
                utilization: 0.42,
                resets_at: "2026-07-13T10:00:00Z".into(),
                prev_utilization: Some(0.31),
            }),
            // seven_day absent → must not produce a bar.
            seven_day_scoped: vec![ScopedUsage {
                model_label: "Fable".into(),
                utilization: 0.07,
                resets_at: "2026-07-19T00:00:00Z".into(),
                prev_utilization: None,
            }],
            ..Default::default()
        };

        let v = claude_account_json(&info);
        assert_eq!(v["email"], "boss@example.com");
        assert_eq!(v["plan"], "Claude Max");
        assert_eq!(v["usageSource"], "anthropic");

        let bars = v["bars"].as_array().expect("bars is an array");
        assert_eq!(bars.len(), 2, "only the two present windows become bars");
        assert_eq!(bars[0]["label"], "5h");
        assert_eq!(bars[0]["utilization"], 0.42);
        assert_eq!(bars[0]["prevUtilization"], 0.31);
        assert_eq!(bars[0]["resetsAt"], "2026-07-13T10:00:00Z");
        assert_eq!(bars[1]["label"], "7d Fable");
        assert_eq!(bars[1]["utilization"], 0.07);
        assert!(bars[1]["prevUtilization"].is_null(), "no previous period → null");
    }

    /// The raw series runs at a ~10s cadence; the phone gets one point per
    /// bucket — the last reading in it — and an empty series stays empty.
    #[test]
    fn downsample_history_keeps_last_sample_per_bucket() {
        use crate::account::UsageHistoryPoint;
        let p = |ts: i64, v: f64| UsageHistoryPoint {
            ts,
            five_hour: Some(v),
            seven_day: None,
            seven_day_sonnet: None,
        };
        let bucket = 1_000;
        let raw = vec![
            p(0, 0.1),
            p(400, 0.2),
            p(999, 0.3), // same bucket as the two above → only this one survives
            p(1_000, 0.4),
            p(1_500, 0.5), // bucket 1 → 0.5 survives
            p(7_200, 0.9), // a gap of empty buckets must not invent points
        ];

        let got = downsample_history(&raw, bucket);
        let pairs: Vec<(i64, Option<f64>)> = got.iter().map(|q| (q.ts, q.five_hour)).collect();
        assert_eq!(
            pairs,
            vec![(999, Some(0.3)), (1_500, Some(0.5)), (7_200, Some(0.9))],
        );

        assert!(downsample_history(&[], bucket).is_empty());
    }

    /// A real-shaped 24h window (~10s cadence, 8k points) must come out small
    /// enough to ship to a phone: at most one point per 5-minute bucket.
    #[test]
    fn downsample_history_caps_a_24h_window() {
        use crate::account::UsageHistoryPoint;
        let raw: Vec<UsageHistoryPoint> = (0..8_640)
            .map(|i| UsageHistoryPoint {
                ts: i * 10_000, // every 10s over 24h
                five_hour: Some(0.5),
                seven_day: None,
                seven_day_sonnet: None,
            })
            .collect();

        let got = downsample_history(&raw, HISTORY_BUCKET_MS);
        assert_eq!(got.len(), 288, "24h / 5min buckets");
    }

    /// Boss's real snapshot file (~8k points in 24h) must come back small enough
    /// to ship: disk-only, so it's fast — but it reads the real home dir, hence
    /// #[ignore]. Run with `-- --ignored`.
    #[test]
    #[ignore = "reads the real ~/.fleet history file"]
    fn usage_history_reply_stays_small_on_real_data() {
        let v = serve_request("usage_history", &Value::Null).expect("usage_history returns rows");
        let rows = v.as_array().expect("an array");
        let bytes = serde_json::to_string(&v).unwrap().len();
        eprintln!("usage_history: {} rows, {} bytes", rows.len(), bytes);
        assert!(rows.len() <= 289, "24h must fit in 5-min buckets, got {}", rows.len());
    }

    /// Pins the relay wiring for the codex occupancy frame: the method is
    /// dispatched and yields a JSON array, never an error. The store logic is
    /// tested in `codex_usage_history`; a `[0, 1]` window guarantees an empty
    /// array regardless of what's on disk, so this stays deterministic in CI.
    #[test]
    fn codex_usage_history_dispatch_returns_empty_array_for_ancient_window() {
        let v = serve_request("codex_usage_history", &json!({ "fromMs": 0, "toMs": 1 }))
            .expect("codex_usage_history dispatches");
        let rows = v.as_array().expect("an array");
        assert!(rows.is_empty(), "no codex points fall in [0, 1] ms");
    }

    /// The real `account_usage` path, driven the way the ws loop drives it:
    /// inside `spawn_blocking` on a multi-thread runtime. This is the
    /// nested-runtime guard — `fetch_account_info_blocking` builds its own tokio
    /// runtime, which is why the handler hands the work to a plain thread. Hits
    /// the network / keychain, so it stays out of CI; run it locally with
    /// `cargo test -p claw-fleet-core account_usage_runs -- --ignored`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore = "network + keychain"]
    async fn account_usage_runs_under_spawn_blocking() {
        let v = tokio::task::spawn_blocking(|| serve_request("account_usage", &Value::Null))
            .await
            .expect("handler must not panic (nested runtime)")
            .expect("account_usage always returns a payload");
        // Claude either resolved or reported why — never both missing.
        assert!(
            v["claude"].is_object() || v["claudeError"].is_string(),
            "expected a claude account or an error, got {v}"
        );
        assert!(v["sources"].is_array(), "sources is always an array");
    }

    /// The pure gzip-gate decision — no global state, safe to run alone.
    #[test]
    fn should_gzip_matrix() {
        let big = SESSIONS_COMPRESS_MIN_BYTES;
        let small = SESSIONS_COMPRESS_MIN_BYTES - 1;
        // Below threshold → never gzip, regardless of client support.
        assert!(!should_gzip(small, true));
        // Large + all clients can inflate → gzip.
        assert!(should_gzip(big, true));
        // Large but a client can't inflate → seal uncompressed.
        assert!(!should_gzip(big, false));
    }

    /// A small payload seals into an opaque `{enc:"box"}` envelope that leaks no
    /// plaintext to the relay, yet a paired peer opens it back to the original.
    #[test]
    fn encode_payload_seals_and_roundtrips_small() {
        // ENC_KEY is a process global the ws thread owns in production; pin it
        // under the shared lock so parallel encode-exercising tests don't race
        // it between this test's seal and open.
        let _guard = fleet_home_lock();
        *ENC_KEY.lock().unwrap() = Some([7u8; 32]);
        let payload = json!({ "event": "reply", "req_id": "r1", "ok": true, "data": 42 });
        let Outbound::Text(text) = encode_payload(&payload);
        // The wire carries no business fields — only the sealed envelope.
        let frame: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(frame["payload"]["enc"], "box");
        assert!(frame["payload"].get("event").is_none(), "no plaintext event on the wire");
        assert!(!text.contains("\"reply\""), "no plaintext leaks into the frame");
        assert!(frame["payload"].get("z").is_none(), "small payload not gzipped");
        // A peer with the same key opens it back to the original.
        assert_eq!(decode_out(&text), payload);
        *ENC_KEY.lock().unwrap() = None;
    }

    /// A large payload is gzipped before sealing (`z:true`) and still opens back
    /// to the exact original — proving compress-then-encrypt round-trips.
    #[test]
    fn encode_payload_gzips_then_seals_large() {
        let _guard = fleet_home_lock();
        *ENC_KEY.lock().unwrap() = Some([7u8; 32]);
        clear_clients();
        // A gzip-capable client on the channel unlocks compression.
        handle_client_payload(&hello_gzip("g1", true));

        use crate::session_launch::NEW_SESSION_ENTRYPOINT;
        let many: Vec<Value> = (0..200)
            .map(|i| json!({"id": format!("session-{i}"), "isSubagent": false,
                            "lastActivityMs": i, "workspaceName": "some-workspace-name",
                            "entrypoint": NEW_SESSION_ENTRYPOINT}))
            .collect();
        let slim = slim_sessions_snapshot(&Value::Array(many));
        let payload = json!({ "event": "sessions", "sessions": slim });
        assert!(
            payload.to_string().len() >= SESSIONS_COMPRESS_MIN_BYTES,
            "test data must exceed the gzip threshold"
        );

        let Outbound::Text(text) = encode_payload(&payload);
        let frame: Value = serde_json::from_str(&text).unwrap();
        assert_eq!(frame["payload"]["enc"], "box");
        assert_eq!(frame["payload"]["z"], true, "large payload gzipped before sealing");
        // Ciphertext is far smaller than the plaintext JSON despite base64.
        assert!(
            text.len() < payload.to_string().len(),
            "gzip-then-seal must shrink a large snapshot"
        );
        // Opens back to the exact original after inflate.
        assert_eq!(decode_out(&text), payload);
        clear_clients();
        *ENC_KEY.lock().unwrap() = None;
    }

    /// The gzip roundtrip helpers used by the transport layer round-trip bytes.
    #[test]
    fn gzip_bytes_roundtrips() {
        let raw = "x".repeat(10_000);
        let gz = gzip_bytes(raw.as_bytes()).expect("gzip");
        assert!(gz.len() < raw.len(), "gzip must shrink repetitive data");
        assert_eq!(gunzip_bytes(&gz).expect("gunzip"), raw.as_bytes());
    }

    #[test]
    fn on_connect_push_emits_current_snapshot_via_provider() {
        use crate::session_launch::NEW_SESSION_ENTRYPOINT;

        // Observable outbound channel so send_out's frames can be inspected.
        let _guard = fleet_home_lock();
        *ENC_KEY.lock().unwrap() = Some([7u8; 32]);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Outbound>();
        *OUT_TX.lock().unwrap() = Some(tx);
        // Simulate an authed channel with one phone attached, freshly connected:
        // dedup hash reset and no prior push to throttle against.
        CONNECTED.store(true, Ordering::SeqCst);
        CLIENTS.store(1, Ordering::SeqCst);
        SESSIONS_LAST_HASH.store(0, Ordering::SeqCst);
        *SESSIONS_LAST_SENT.lock().unwrap() = None;

        // No provider (the `fleet serve` fallback, and the pre-fix desktop
        // behaviour): a connect pushes nothing — the phone would sit blank until
        // the next scan-driven change. This is exactly the bug being fixed.
        *SNAPSHOT_PROVIDER.lock().unwrap() = None;
        push_snapshot_on_connect();
        assert!(rx.try_recv().is_err(), "no provider → no on-connect push (pre-fix state)");

        // With a provider registered (the desktop LocalBackend path), connecting
        // emits the current snapshot immediately, no scan tick required.
        set_snapshot_provider(|| {
            Some(json!([{
                "id": "s1", "isSubagent": false, "lastActivityMs": 1,
                "workspaceName": "w", "entrypoint": NEW_SESSION_ENTRYPOINT
            }]))
        });
        push_snapshot_on_connect();
        let out = rx.try_recv().expect("on-connect push emitted a frame");
        let Outbound::Text(text) = out;
        // The payload crosses the relay sealed; a paired peer opens it.
        assert_eq!(decode_out(&text)["event"], "sessions");

        // Reset shared globals so sibling tests start from a known state.
        *SNAPSHOT_PROVIDER.lock().unwrap() = None;
        *OUT_TX.lock().unwrap() = None;
        *ENC_KEY.lock().unwrap() = None;
        CONNECTED.store(false, Ordering::SeqCst);
        CLIENTS.store(0, Ordering::SeqCst);
        SESSIONS_LAST_HASH.store(0, Ordering::SeqCst);
        *SESSIONS_LAST_SENT.lock().unwrap() = None;
    }

    /// The pure per-id diff — new/changed sessions become `upsert`, vanished
    /// ids become `remove`, unchanged ones are omitted. No globals, safe alone.
    #[test]
    fn diff_snapshot_keys_upsert_and_remove_by_id() {
        use crate::session_launch::NEW_SESSION_ENTRYPOINT;
        let ep = NEW_SESSION_ENTRYPOINT;
        let base_list = slim_sessions_snapshot(&json!([
            {"id": "s1", "lastActivityMs": 3, "entrypoint": ep},
            {"id": "s2", "lastActivityMs": 2, "entrypoint": ep},
            {"id": "s3", "lastActivityMs": 1, "entrypoint": ep},
        ]));
        let baseline = per_id_hashes(&base_list);

        // s1 unchanged, s2 changed (activity bump), s4 new, s3 gone.
        let next = slim_sessions_snapshot(&json!([
            {"id": "s1", "lastActivityMs": 3, "entrypoint": ep},
            {"id": "s2", "lastActivityMs": 9, "entrypoint": ep},
            {"id": "s4", "lastActivityMs": 4, "entrypoint": ep},
        ]));
        let (upsert, mut remove) = diff_snapshot(&baseline, &next);

        let mut up_ids: Vec<&str> =
            upsert.iter().filter_map(|s| s["id"].as_str()).collect();
        up_ids.sort_unstable();
        assert_eq!(up_ids, vec!["s2", "s4"], "only changed + new ship as upsert");
        remove.sort();
        assert_eq!(remove, vec!["s3".to_string()], "vanished id ships as remove");

        // Identical snapshot → nothing to send.
        let (u2, r2) = diff_snapshot(&per_id_hashes(&next), &next);
        assert!(u2.is_empty() && r2.is_empty(), "no change → empty delta");
    }

    /// The delta capability gate mirrors the gzip gate: every live client must
    /// opt in, and an empty registry never deltas. Shares the global registry,
    /// so it runs under the home lock and cleans up.
    #[test]
    fn all_clients_support_delta_gate() {
        let _guard = fleet_home_lock();
        clear_clients();
        assert!(!all_clients_support_delta(), "empty registry → never delta");
        handle_client_payload(&hello_delta("d1", true));
        assert!(all_clients_support_delta(), "sole delta-capable client → delta");
        // A legacy client (no flag → default false) vetoes deltas channel-wide.
        handle_client_payload(&hello("legacy", "old PWA", false));
        assert!(!all_clients_support_delta(), "one legacy client forces full snapshots");
        clear_clients();
    }

    /// End-to-end through the sealed channel: the first push after a baseline
    /// reset is a full `sessions` snapshot; a following change ships as a
    /// `sessions_delta` carrying only the changed/added rows and removed ids;
    /// and a legacy client on the channel forces a full snapshot again.
    #[test]
    fn publish_sessions_emits_full_then_delta() {
        use crate::session_launch::NEW_SESSION_ENTRYPOINT;
        let ep = NEW_SESSION_ENTRYPOINT;
        let _guard = fleet_home_lock();
        *ENC_KEY.lock().unwrap() = Some([7u8; 32]);
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Outbound>();
        *OUT_TX.lock().unwrap() = Some(tx);
        CONNECTED.store(true, Ordering::SeqCst);
        CLIENTS.store(1, Ordering::SeqCst);
        clear_clients();
        reset_sessions_dedup();
        *SESSIONS_LAST_SENT.lock().unwrap() = None;
        handle_client_payload(&hello_delta("d1", true));

        // First push after a reset → full snapshot (no baseline to diff against).
        publish_sessions(&json!([
            {"id": "s1", "isSubagent": false, "lastActivityMs": 3, "entrypoint": ep},
            {"id": "s2", "isSubagent": false, "lastActivityMs": 2, "entrypoint": ep},
            {"id": "s3", "isSubagent": false, "lastActivityMs": 1, "entrypoint": ep},
        ]));
        let Outbound::Text(t1) = rx.try_recv().expect("first push emits a frame");
        let full = decode_out(&t1);
        assert_eq!(full["event"], "sessions", "first push is a full snapshot");
        assert_eq!(full["sessions"].as_array().unwrap().len(), 3);

        // Second push, one changed + one new + one gone → a delta with just those.
        *SESSIONS_LAST_SENT.lock().unwrap() = None; // bypass the 2s throttle in-test
        publish_sessions(&json!([
            {"id": "s1", "isSubagent": false, "lastActivityMs": 3, "entrypoint": ep},
            {"id": "s2", "isSubagent": false, "lastActivityMs": 9, "entrypoint": ep},
            {"id": "s4", "isSubagent": false, "lastActivityMs": 4, "entrypoint": ep},
        ]));
        let Outbound::Text(t2) = rx.try_recv().expect("second push emits a frame");
        let delta = decode_out(&t2);
        assert_eq!(delta["event"], "sessions_delta", "second push is a delta");
        let mut up_ids: Vec<&str> =
            delta["upsert"].as_array().unwrap().iter().filter_map(|s| s["id"].as_str()).collect();
        up_ids.sort_unstable();
        assert_eq!(up_ids, vec!["s2", "s4"]);
        assert_eq!(delta["remove"].as_array().unwrap(), &vec![json!("s3")]);

        // A legacy client joins → the gate closes, next push is full again.
        handle_client_payload(&hello("legacy", "old PWA", false));
        *SESSIONS_LAST_SENT.lock().unwrap() = None;
        publish_sessions(&json!([
            {"id": "s2", "isSubagent": false, "lastActivityMs": 10, "entrypoint": ep},
        ]));
        let Outbound::Text(t3) = rx.try_recv().expect("third push emits a frame");
        assert_eq!(decode_out(&t3)["event"], "sessions", "legacy client forces full");

        // Reset shared globals for sibling tests.
        clear_clients();
        reset_sessions_dedup();
        *OUT_TX.lock().unwrap() = None;
        *ENC_KEY.lock().unwrap() = None;
        CONNECTED.store(false, Ordering::SeqCst);
        CLIENTS.store(0, Ordering::SeqCst);
        *SESSIONS_LAST_SENT.lock().unwrap() = None;
    }

    fn hello_delta(client_id: &str, supports_delta: bool) -> Value {
        json!({
            "event": "client_hello",
            "clientId": client_id,
            "label": "x",
            "platform": "harmony",
            "pushSubscribed": false,
            "supportsDelta": supports_delta,
        })
    }

    /// Regression: the repo handlers must NOT double-wrap their core `Result`.
    /// `serde_json::to_value(repo_detail(..))` serialises the `Result` itself as
    /// serde's externally-tagged enum (`{"Ok": {..}}` / `{"Err": ".."}`), so a
    /// core error was buried inside a *successful* `{"Err": ".."}` frame and a
    /// success arrived as `{"Ok": {..}}` — the client's `detail.worktrees` was
    /// then `undefined` and both mobile clients crashed on the detail page.
    /// A fake root is never a known workspace, so `repo_detail` errors; the
    /// handler must surface that as a protocol `Err`, not a wrapped `Ok`.
    #[test]
    fn repo_handlers_propagate_core_error_not_wrapped_ok() {
        let params = json!({ "root": "/nonexistent/fleet-repo-xyz" });
        assert!(
            serve_repo_detail(&params).is_err(),
            "repo_detail handler must propagate the core Err, not a wrapped Ok",
        );
        assert!(
            serve_repo_push(&params).is_err(),
            "repo_push handler must propagate the core Err, not a wrapped Ok",
        );
        assert!(
            serve_repo_pull(&params).is_err(),
            "repo_pull handler must propagate the core Err, not a wrapped Ok",
        );
    }

    // ── transcript slimming keeps the fields the redesigned detail view reads ──

    /// Regression: `isMeta`/`sourceToolUseID` were missing from `TAIL_MSG_FIELDS`,
    /// so the phone's `isMetaRow` (which requires `msg.isMeta`) never matched over
    /// the relay — SKILL.md injections rendered as full-length user bubbles
    /// instead of folding into a MetaFoldCard.
    #[test]
    fn slim_tail_keeps_meta_flags() {
        let msgs = vec![json!({
            "type": "user",
            "uuid": "u1",
            "isMeta": true,
            "sourceToolUseID": "toolu_abc",
            "cwd": "/somewhere",
            "message": { "role": "user", "content": "injected SKILL.md body" }
        })];
        let slim = slim_tail_messages(msgs);
        assert_eq!(slim[0]["isMeta"], json!(true), "isMeta must survive slimming");
        assert_eq!(
            slim[0]["sourceToolUseID"],
            json!("toolu_abc"),
            "sourceToolUseID must survive slimming"
        );
        assert!(slim[0].get("cwd").is_none(), "bookkeeping fields stay stripped");
    }

    /// The redesigned view shows one `↑in ↓out · model` line per turn and uses
    /// `stop_reason` to tell whether a work run terminated. Only the two token
    /// counters survive from `usage` — cache bookkeeping stays stripped.
    #[test]
    fn slim_tail_keeps_turn_usage_fields() {
        let msgs = vec![json!({
            "type": "assistant",
            "uuid": "a1",
            "message": {
                "role": "assistant",
                "model": "claude-opus-4-8",
                "stop_reason": "end_turn",
                "usage": {
                    "input_tokens": 1200,
                    "output_tokens": 340,
                    "cache_read_input_tokens": 99999,
                    "cache_creation_input_tokens": 5
                },
                "content": [{ "type": "text", "text": "done" }]
            }
        })];
        let slim = slim_tail_messages(msgs);
        let m = &slim[0]["message"];
        assert_eq!(m["model"], json!("claude-opus-4-8"));
        assert_eq!(m["stop_reason"], json!("end_turn"));
        assert_eq!(m["usage"]["input_tokens"], json!(1200));
        assert_eq!(m["usage"]["output_tokens"], json!(340));
        assert!(
            m["usage"].get("cache_read_input_tokens").is_none(),
            "cache counters are not rendered and must stay stripped"
        );
    }

    /// Error badges on tool chips need the `tool_result` block's `is_error` bit;
    /// the result `content` itself stays stripped (fetched on demand instead).
    #[test]
    fn slim_tail_keeps_tool_result_error_bit() {
        let msgs = vec![json!({
            "type": "user",
            "uuid": "u2",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "tool_use_id": "toolu_x",
                    "is_error": true,
                    "content": [{ "type": "text", "text": "boom stack trace ..." }]
                }]
            }
        })];
        let slim = slim_tail_messages(msgs);
        let block = &slim[0]["message"]["content"][0];
        assert_eq!(block["tool_use_id"], json!("toolu_x"));
        assert_eq!(block["is_error"], json!(true), "is_error must survive slimming");
        assert!(
            block.get("content").is_none(),
            "tool_result bodies stay stripped from the skeleton stream"
        );
    }

    /// `toolUseResult` is stripped as a whole, but its stats survive as a tiny
    /// `_digest` on the record's `tool_result` block — the phone's stat chips
    /// without the bodies.
    #[test]
    fn slim_tail_digests_tool_result_stats() {
        let record = |tool_use_result: Value| {
            json!({
                "type": "user",
                "uuid": "u3",
                "message": { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "toolu_d", "content": "big blob" }
                ]},
                "toolUseResult": tool_use_result
            })
        };

        // Edit/Write → ± line counts from structuredPatch.
        let slim = slim_tail_messages(vec![record(json!({
            "filePath": "/a/b.rs",
            "originalFile": "fn main() {}\n",
            "structuredPatch": [
                { "oldStart": 1, "oldLines": 2, "newStart": 1, "newLines": 3,
                  "lines": [" ctx", "+new line", "+another", "-old line"] }
            ]
        }))]);
        let d = &slim[0]["message"]["content"][0]["_digest"];
        assert_eq!(d["added"], json!(2));
        assert_eq!(d["removed"], json!(1));
        assert!(
            slim[0].get("toolUseResult").is_none(),
            "raw toolUseResult still stripped"
        );

        // Bash → stdout/stderr line counts.
        let slim = slim_tail_messages(vec![record(json!({
            "stdout": "one\ntwo\n\n", "stderr": "", "interrupted": false
        }))]);
        let d = &slim[0]["message"]["content"][0]["_digest"];
        assert_eq!(d["stdoutLines"], json!(2));
        assert_eq!(d["stderrLines"], json!(0));
        assert!(d.get("interrupted").is_none(), "false flags stay omitted");

        // Agent → status + totals.
        let slim = slim_tail_messages(vec![record(json!({
            "status": "completed", "totalDurationMs": 120000,
            "totalTokens": 45000, "totalToolUseCount": 12, "prompt": "long ..."
        }))]);
        let d = &slim[0]["message"]["content"][0]["_digest"];
        assert_eq!(d["agentStatus"], json!("completed"));
        assert_eq!(d["durationMs"], json!(120000));
        assert_eq!(d["tokens"], json!(45000));
        assert_eq!(d["toolUses"], json!(12));

        // Grep → files/matches; TodoWrite → done/total.
        let slim = slim_tail_messages(vec![record(json!({
            "numFiles": 3, "numMatches": 17, "truncated": true, "filenames": ["a", "b", "c"]
        }))]);
        let d = &slim[0]["message"]["content"][0]["_digest"];
        assert_eq!(d["files"], json!(3));
        assert_eq!(d["matches"], json!(17));
        assert_eq!(d["truncated"], json!(true));

        let slim = slim_tail_messages(vec![record(json!({
            "oldTodos": [], "newTodos": [
                { "content": "x", "status": "completed" },
                { "content": "y", "status": "pending" }
            ]
        }))]);
        let d = &slim[0]["message"]["content"][0]["_digest"];
        assert_eq!(d["todoDone"], json!(1));
        assert_eq!(d["todoTotal"], json!(2));

        // Errored call: toolUseResult degrades to a bare string → no digest.
        let slim = slim_tail_messages(vec![record(json!("Error: something broke"))]);
        assert!(
            slim[0]["message"]["content"][0].get("_digest").is_none(),
            "string toolUseResult yields no digest"
        );
    }

    /// Image blocks ship a server-side JPEG thumbnail instead of the original
    /// base64 (or nothing renderable when the source doesn't decode), and
    /// tool_result-embedded screenshots surface as a capped `_thumbs` list
    /// while the result body stays stripped.
    #[test]
    fn slim_tail_thumbnails_images() {
        use base64::Engine as _;
        // A decodable ~600×400 gradient PNG standing in for a screenshot.
        let mut buf = image::RgbImage::new(600, 400);
        for (x, y, px) in buf.enumerate_pixels_mut() {
            *px = image::Rgb([(x / 4) as u8, (y / 2) as u8, ((x + y) / 8) as u8]);
        }
        let png_b64 = base64::engine::general_purpose::STANDARD.encode(png_of(buf));
        let img_block = |data: &str| {
            json!({ "type": "image",
                    "source": { "type": "base64", "media_type": "image/png", "data": data } })
        };

        // Top-level image block → jpeg thumb under the hard cap, marked _thumb.
        let slim = slim_tail_messages(vec![json!({
            "type": "user", "uuid": "img1",
            "message": { "role": "user", "content": [img_block(&png_b64)] }
        })]);
        let block = &slim[0]["message"]["content"][0];
        assert_eq!(block["_thumb"], json!(true));
        assert_eq!(block["source"]["media_type"], json!("image/jpeg"));
        let thumb_bytes = base64::engine::general_purpose::STANDARD
            .decode(block["source"]["data"].as_str().expect("thumb data"))
            .expect("thumb decodes");
        assert!(
            thumb_bytes.len() <= TAIL_THUMB_HARD_CAP_BYTES,
            "thumb ({}B) must respect the hard cap",
            thumb_bytes.len()
        );
        image::load_from_memory(&thumb_bytes).expect("thumb is a valid image");

        // Undecodable source → old type-only slim form, no source shipped.
        let slim = slim_tail_messages(vec![json!({
            "type": "user", "uuid": "img2",
            "message": { "role": "user", "content": [img_block("AAAAAAAA")] }
        })]);
        let block = &slim[0]["message"]["content"][0];
        assert!(block.get("source").is_none(), "junk image ships no source");
        assert!(block.get("_thumb").is_none());

        // tool_result screenshots → capped `_thumbs`, body still stripped.
        let many: Vec<Value> = (0..5).map(|_| img_block(&png_b64)).collect();
        let slim = slim_tail_messages(vec![json!({
            "type": "user", "uuid": "img3",
            "message": { "role": "user", "content": [{
                "type": "tool_result", "tool_use_id": "toolu_img",
                "content": many
            }] }
        })]);
        let block = &slim[0]["message"]["content"][0];
        assert!(block.get("content").is_none(), "result body stays stripped");
        let thumbs = block["_thumbs"].as_array().expect("thumbs present");
        assert_eq!(thumbs.len(), TAIL_THUMB_MAX_PER_RESULT, "thumb count capped");
    }

    /// `tool_detail` is the tap-to-expand counterpart of the slim stream: full
    /// input + result for one tool_use_id, previews truncated unless `full`,
    /// screenshots re-encoded into `images` instead of raw base64.
    #[test]
    fn request_tool_detail_expands_one_tool_call() {
        with_temp_home(|| {
            let big_stdout = "y".repeat(10_000);
            let path = write_jsonl(
                "detail.jsonl",
                &[
                    json!({
                        "type": "assistant", "uuid": "a1",
                        "message": { "role": "assistant", "content": [
                            { "type": "tool_use", "id": "toolu_det", "name": "Bash",
                              "input": { "command": "make", "description": "build it" } }
                        ]}
                    }),
                    json!({
                        "type": "user", "uuid": "u1",
                        "message": { "role": "user", "content": [
                            { "type": "tool_result", "tool_use_id": "toolu_det",
                              "content": big_stdout }
                        ]},
                        "toolUseResult": { "stdout": big_stdout, "stderr": "", "interrupted": false }
                    }),
                ],
            );

            // Default: truncated previews + flag.
            let d = request_ok("tool_detail", json!({ "path": path, "tool_use_id": "toolu_det" }));
            assert_eq!(d["name"], json!("Bash"));
            assert_eq!(d["input"]["command"], json!("make"), "full input comes back");
            assert_eq!(d["truncated"], json!(true));
            let preview = d["content"].as_str().expect("content preview");
            assert!(preview.len() < 3000, "content must be a preview, got {}B", preview.len());
            assert!(preview.contains("Fleet truncated"));

            // full=true returns the whole body.
            let d = request_ok(
                "tool_detail",
                json!({ "path": path, "tool_use_id": "toolu_det", "full": true }),
            );
            assert!(d.get("truncated").is_none());
            assert_eq!(d["content"].as_str().unwrap().len(), 10_000);
            assert_eq!(d["toolUseResult"]["stdout"].as_str().unwrap().len(), 10_000);

            // Unknown id is a protocol error, not an empty reply.
            assert!(serve_tool_detail(
                &json!({ "path": path, "tool_use_id": "toolu_missing" })
            )
            .is_err());
        });
    }

    /// Screenshots inside a tool_result come back as re-encoded `images`, never
    /// as raw base64 in `content` — and never cut to a useless text preview.
    #[test]
    fn request_tool_detail_reencodes_result_images() {
        use base64::Engine as _;
        with_temp_home(|| {
            let mut buf = image::RgbImage::new(600, 400);
            for (x, y, px) in buf.enumerate_pixels_mut() {
                *px = image::Rgb([(x / 4) as u8, (y / 2) as u8, 0]);
            }
            let png_b64 = base64::engine::general_purpose::STANDARD.encode(png_of(buf));
            let path = write_jsonl(
                "detail-img.jsonl",
                &[json!({
                    "type": "user", "uuid": "u1",
                    "message": { "role": "user", "content": [
                        { "type": "tool_result", "tool_use_id": "toolu_shot", "content": [
                            { "type": "text", "text": "screenshot taken" },
                            { "type": "image",
                              "source": { "type": "base64", "media_type": "image/png",
                                          "data": png_b64 } }
                        ]}
                    ]}
                })],
            );

            let d = request_ok("tool_detail", json!({ "path": path, "tool_use_id": "toolu_shot" }));
            let images = d["images"].as_array().expect("images list");
            assert_eq!(images.len(), 1);
            let bytes = base64::engine::general_purpose::STANDARD
                .decode(images[0]["data"].as_str().unwrap())
                .expect("image decodes");
            assert!(bytes.len() <= DECISION_ASSET_HARD_CAP_BYTES);
            image::load_from_memory(&bytes).expect("valid image");
            // The raw base64 is gone from the content blocks.
            let blocks = d["content"].as_array().expect("content blocks");
            assert_eq!(blocks[0]["text"], json!("screenshot taken"));
            assert!(blocks[1].get("source").is_none(), "raw base64 stripped");
        });
    }
}
