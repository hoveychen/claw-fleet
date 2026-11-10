//! The relay's `http` method: a whitelisted HTTP tunnel onto this host's own
//! `fleet serve` routes.
//!
//! The desktop web bundle (the same `claw-fleet-desktop` dist `fleet webui`
//! serves) speaks `fleet serve`'s HTTP routes. Opened from the relay's origin it
//! has no route to this Mac except the E2E-encrypted relay channel, so a
//! Service Worker on the browser side wraps each fetch into one relay request:
//!
//! ```text
//! params: { method, path, query: {k: string} | null, body: <json> | null,
//!           bodyB64: string | null, contentType: string | null }
//! reply:  { status, contentType, body: string | null, bodyB64: string | null }
//! ```
//!
//! This module answers it by replaying the request as a loopback HTTP call to
//! a hooks server in this process ([`register_loopback_target`]), so every
//! route behaves exactly as it does for `fleet webui` — there is no second
//! mapping table from desktop commands to relay methods to keep in sync.
//!
//! **The boundary is [`HTTP_TUNNEL_ROUTES`].** The relay channel is reachable by
//! any paired phone or browser, and `fleet serve`'s admin surface includes
//! command execution, settings, guidance and skill writes. Only the listed
//! `(method, path)` pairs are forwarded; everything else is answered 403
//! without a loopback call being made.
//!
//! Where the loopback target comes from: `fleet serve` / `fleet webui` register
//! their own bound port and token as soon as they listen. The desktop app runs
//! no hooks server, so the first tunnelled request there starts an embedded,
//! loopback-only one ([`crate::hooks_server::spawn_embedded_loopback`]) with a
//! random token that never leaves the process. `~/.fleet/port` is deliberately
//! not consulted: it is whatever serve process wrote it last, often long dead,
//! and pointing the tunnel at another process would bypass the relay role
//! arbitration that makes this process the one answering the phone.

use std::sync::Mutex;
use std::time::Duration;

use base64::Engine as _;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Value};

/// Largest response body the tunnel hands back, before encoding.
///
/// Same "one file in one frame" ceiling as
/// [`crate::mobile_relay::MAX_ARTIFACT_FRAME_BYTES`]: 16 MiB base64-encodes to
/// ~21 MiB, which still fits the relay's 32 MiB message cap once sealed. A
/// larger body is answered 413 instead of being sent.
pub const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

/// Largest request body the tunnel forwards, decoded. Matches the relay's
/// attachment upload cap ([`crate::mobile_relay::MAX_UPLOAD_BYTES`]).
pub const MAX_REQUEST_BYTES: usize = crate::mobile_relay::MAX_UPLOAD_BYTES as usize;

/// Per-request ceiling on the loopback call. Longer than any read route takes
/// on a busy host (a cold `/sessions` scan is seconds), short enough that a
/// wedged route does not pin a relay worker indefinitely.
const LOOPBACK_TIMEOUT: Duration = Duration::from_secs(60);

/// Every `(method, path)` the tunnel forwards. A pattern ending in `/*`
/// matches that prefix plus a non-empty remainder; a `*` segment anywhere else
/// matches exactly one non-empty segment; anything else matches exactly.
///
/// Scope is the "lite" desktop web: what the bundle needs to boot, the session
/// list and transcripts, pending decision cards and answering them, and the
/// images and wiki assets cards and docs embed — the capability set the phone
/// already has over relay methods. Read-only GETs with a phone equivalent are
/// in; routes that write files, change settings / guidance / skills / guard
/// config, spawn processes or run shells are out. Note that "GET" is not a
/// proxy for "read-only" here: `/interrupt`, `/stop` and
/// `/daily_report/generate` are GETs with side effects, which is why this is an
/// explicit list rather than a method rule.
pub const HTTP_TUNNEL_ROUTES: &[(&str, &str)] = &[
    // ── Boot ────────────────────────────────────────────────────────────
    ("GET", crate::routes::HEALTH),
    ("GET", crate::routes::HOST_FEATURES),
    ("GET", crate::routes::HOST_IDENTITY),
    ("GET", crate::routes::MODEL_CATALOG),
    ("GET", crate::routes::DSH_MODELS),
    ("GET", crate::routes::CHAT_WORKSPACE),
    // ── Library (read-only; the phone already browses it) ──────────────
    ("GET", crate::routes::LIBRARY_TREE),
    ("GET", crate::routes::LIBRARY_FOLDER_ITEMS),
    ("GET", crate::routes::LIBRARY_SEARCH),
    ("GET", crate::routes::LIBRARY_INGEST_BATCHES),
    ("GET", crate::routes::LIBRARY_INGEST_BATCH),
    // ── Session list and transcripts ────────────────────────────────────
    ("GET", crate::routes::SESSIONS),
    ("GET", crate::routes::SESSIONS_SNAPSHOT),
    ("GET", crate::routes::MESSAGES),
    ("GET", crate::routes::TAIL),
    ("GET", crate::routes::FILE_SIZE),
    ("GET", crate::routes::TOOL_RESULT),
    ("GET", crate::routes::LIVE_THINKING),
    ("GET", crate::routes::SEARCH),
    ("GET", crate::routes::HANDOFF_CHAIN),
    ("GET", crate::routes::WORKFLOW_TREES),
    ("GET", crate::routes::TASK_PLANS),
    ("GET", crate::routes::PLAN_FOREST),
    ("GET", crate::routes::SESSION_DECISIONS),
    ("GET", crate::routes::SESSION_IMAGES),
    ("GET", crate::routes::SESSION_IMAGE),
    ("GET", crate::routes::USER_ATTACHMENT),
    ("GET", crate::routes::SESSION_NOTES),
    ("GET", crate::routes::SESSION_NOTE),
    ("GET", crate::routes::SESSION_NOTES_SEARCH),
    // ── Usage readouts (phone: today_usage / token_breakdown / …) ───────
    ("GET", crate::routes::TODAY_USAGE),
    ("GET", crate::routes::TODAY_USAGE_BREAKDOWN),
    ("GET", crate::routes::USAGE_RANGE_BREAKDOWN),
    ("GET", crate::routes::USAGE_HISTORY),
    ("GET", crate::routes::CODEX_USAGE_HISTORY),
    ("GET", crate::routes::TOKEN_BREAKDOWN),
    ("GET", crate::routes::DSH_TOKEN_BREAKDOWN),
    ("GET", crate::routes::DSH_SESSION_COST),
    // ── Pending decision cards ──────────────────────────────────────────
    ("GET", crate::routes::DECISIONS_PENDING),
    ("GET", crate::routes::GUARD_PENDING),
    ("GET", crate::routes::ELICITATION_PENDING),
    ("GET", crate::routes::FLEET_ASK_PENDING),
    ("GET", crate::routes::PLAN_APPROVAL_PENDING),
    ("GET", crate::routes::A2UI_RENDER_PENDING),
    ("GET", crate::routes::PERMISSION_PROMPT_PENDING),
    // ── Answering a card (phone: decision_answer) ───────────────────────
    ("POST", crate::routes::GUARD_RESPOND),
    ("POST", crate::routes::ELICITATION_RESPOND),
    ("POST", crate::routes::FLEET_ASK_RESPOND),
    ("POST", crate::routes::PLAN_APPROVAL_RESPOND),
    ("POST", crate::routes::A2UI_RENDER_RESPOND),
    ("POST", crate::routes::PERMISSION_PROMPT_RESPOND),
    // ── Images in cards, wiki docs and their assets ─────────────────────
    ("GET", crate::routes::DECISION_ASSET),
    ("GET", "/decision_asset/*"),
    ("GET", crate::routes::WIKI_DOCS),
    ("GET", crate::routes::WIKI_DOC),
    ("GET", crate::routes::WIKI_FILE),
    ("GET", crate::routes::WIKI_SEARCH),
    ("GET", "/wiki_asset/*"),
    // ── Desktop web boot / panels — read-only, each with a phone equivalent
    ("GET", crate::routes::PROCS),
    ("GET", crate::routes::SOURCES_CONFIG),
    ("GET", "/sources/*/usage"),
    ("GET", crate::routes::SOURCES_CLAUDE_ACCOUNT),
    ("GET", crate::routes::ARTIFACTS),
    ("GET", crate::routes::ARTIFACT_BLOB),
    ("GET", crate::routes::SESSION_EXPLAINS),
    ("GET", crate::routes::DAILY_REPORT_ATTENTION),
    ("GET", crate::routes::DAILY_REPORT_STATS),
    ("GET", crate::routes::HOOKS_PLAN),
    ("GET", crate::routes::SETUP_STATUS),
    ("GET", crate::routes::PROJECTS),
    ("GET", crate::routes::SCRATCHPAD_DIR),
    // Attachments for answering a card (phone: upload_attachment). Writes
    // only into the attachment staging directory.
    ("POST", crate::routes::ELICITATION_UPLOAD),
];

/// Where tunnelled requests go: a hooks server in this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoopbackTarget {
    /// Host to connect to — loopback unless the server bound one specific
    /// non-wildcard address.
    pub host: String,
    pub port: u16,
    /// Admin token to present; `None` when the server runs with auth off
    /// (`fleet webui`).
    pub token: Option<String>,
}

static TARGET: Mutex<Option<LoopbackTarget>> = Mutex::new(None);

/// Record this process's own hooks server as the tunnel's target. Called by
/// `hooks_server::serve` once it is listening (and by the embedded server).
pub fn register_loopback_target(bind_host: &str, port: u16, token: &str, auth_disabled: bool) {
    let host = connect_host(bind_host);
    let token = (!auth_disabled && !token.is_empty()).then(|| token.to_string());
    *TARGET.lock().unwrap_or_else(|e| e.into_inner()) = Some(LoopbackTarget { host, port, token });
}

/// A wildcard bind is reached over loopback; a specific address as itself.
fn connect_host(bind_host: &str) -> String {
    match bind_host {
        "" | "0.0.0.0" | "::" | "[::]" => "127.0.0.1".to_string(),
        h => h.to_string(),
    }
}

/// The registered target, starting the embedded server when there is none.
///
/// Holds the lock across the start so two concurrent first requests do not
/// each spin up a server.
fn resolve_target() -> Result<LoopbackTarget, String> {
    let mut guard = TARGET.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(t) = guard.as_ref() {
        return Ok(t.clone());
    }
    let started = crate::hooks_server::spawn_embedded_loopback()?;
    *guard = Some(started.clone());
    Ok(started)
}

/// Whether `(method, path)` is on [`HTTP_TUNNEL_ROUTES`]. `path` must already
/// have passed [`path_is_clean`].
pub fn is_allowed(method: &str, path: &str) -> bool {
    HTTP_TUNNEL_ROUTES
        .iter()
        .any(|(m, pattern)| m.eq_ignore_ascii_case(method) && pattern_matches(pattern, path))
}

fn pattern_matches(pattern: &str, path: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) if prefix.ends_with('/') && !prefix.contains('*') => {
            path.len() > prefix.len() && path.starts_with(prefix)
        }
        _ => segments_match(pattern, path),
    }
}

fn segments_match(pattern: &str, path: &str) -> bool {
    let pat: Vec<&str> = pattern.split('/').collect();
    let got: Vec<&str> = path.split('/').collect();
    pat.len() == got.len()
        && pat
            .iter()
            .zip(&got)
            .all(|(p, g)| if *p == "*" { !g.is_empty() } else { p == g })
}

/// A path the tunnel is willing to put on a request line: absolute, no query
/// or fragment (those travel in `query`), no `..` segment in either raw or
/// percent-encoded form, no backslash or control/space characters.
fn path_is_clean(path: &str) -> bool {
    if !path.starts_with('/') {
        return false;
    }
    if path
        .chars()
        .any(|c| c.is_control() || c == ' ' || c == '?' || c == '#' || c == '\\')
    {
        return false;
    }
    let lowered = path.to_ascii_lowercase();
    if lowered.contains("%2e") || lowered.contains("%5c") {
        // Encoded dots and backslashes are how a prefix match gets smuggled
        // past; none of the whitelisted routes needs them in the path itself.
        return false;
    }
    if lowered.contains("%2f") && !encoded_slash_only_in_wiki_slug(&lowered) {
        return false;
    }
    !path.split('/').any(|seg| seg == ".." || seg == ".")
}

/// The one place an encoded `/` is legitimate: the slug segment of
/// `/wiki_asset/<slug>/<version>/<rel…>`. Wiki slugs are `<project>/<folder>/
/// <name>`, and `wikiFileUrl` encodes the slug's own slashes so the server's
/// `splitn(3, '/')` keeps it in one part — rejecting `%2F` there broke every
/// wiki HTML preview over the relay (found in the Tauri e2e run, 2026-11-10).
/// Anywhere else — the version, the relpath, any other route — it stays out.
fn encoded_slash_only_in_wiki_slug(lowered: &str) -> bool {
    let Some(tail) = lowered.strip_prefix(crate::routes::WIKI_ASSET_PREFIX) else {
        return false;
    };
    match tail.split_once('/') {
        Some((_slug, rest)) => !rest.contains("%2f"),
        None => false,
    }
}

/// `encodeURIComponent`'s unreserved set — what the browser build uses for
/// query values, and what the route handlers' `percent_decode_str` undoes.
const QUERY_VALUE: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'!')
    .remove(b'~')
    .remove(b'*')
    .remove(b'\'')
    .remove(b'(')
    .remove(b')');

/// A validated tunnel request.
#[derive(Debug)]
struct TunnelRequest {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    body: Option<Vec<u8>>,
    content_type: Option<String>,
}

/// A reply in the wire shape, with a JSON `{error}` body.
fn error_reply(status: u16, message: &str) -> Value {
    json!({
        "status": status,
        "contentType": "application/json",
        "body": json!({ "error": message }).to_string(),
        "bodyB64": Value::Null,
    })
}

fn parse(params: &Value) -> Result<TunnelRequest, Value> {
    let bad = |m: &str| error_reply(400, m);
    let method = params
        .get("method")
        .and_then(Value::as_str)
        .map(str::to_ascii_uppercase)
        .ok_or_else(|| bad("missing method"))?;
    let path = params
        .get("path")
        .and_then(Value::as_str)
        .ok_or_else(|| bad("missing path"))?
        .to_string();
    if !path_is_clean(&path) {
        return Err(bad("invalid path"));
    }

    let mut query = Vec::new();
    match params.get("query") {
        None | Some(Value::Null) => {}
        Some(Value::Object(map)) => {
            for (k, v) in map {
                let v = match v {
                    Value::Null => continue,
                    Value::String(s) => s.clone(),
                    Value::Number(n) => n.to_string(),
                    Value::Bool(b) => b.to_string(),
                    _ => return Err(bad("query values must be strings")),
                };
                query.push((k.clone(), v));
            }
        }
        Some(_) => return Err(bad("query must be an object")),
    }

    let json_body = params.get("body").filter(|v| !v.is_null());
    let b64_body = params.get("bodyB64").filter(|v| !v.is_null());
    let mut content_type = params
        .get("contentType")
        .and_then(Value::as_str)
        .map(str::to_string);
    let body = match (json_body, b64_body) {
        (Some(_), Some(_)) => return Err(bad("at most one of body and bodyB64")),
        (Some(v), None) => {
            content_type.get_or_insert_with(|| "application/json".to_string());
            Some(serde_json::to_vec(v).map_err(|e| bad(&e.to_string()))?)
        }
        (None, Some(v)) => {
            let s = v.as_str().ok_or_else(|| bad("bodyB64 must be a string"))?;
            Some(
                base64::engine::general_purpose::STANDARD
                    .decode(s)
                    .map_err(|e| bad(&format!("bodyB64 is not base64: {e}")))?,
            )
        }
        (None, None) => None,
    };
    if body.as_ref().is_some_and(|b| b.len() > MAX_REQUEST_BYTES) {
        return Err(error_reply(
            413,
            &format!("request body over the {MAX_REQUEST_BYTES}-byte tunnel limit"),
        ));
    }
    Ok(TunnelRequest {
        method,
        path,
        query,
        body,
        content_type,
    })
}

/// Content types that come back as UTF-8 text in `body` rather than base64.
fn is_textual(content_type: &str) -> bool {
    let essence = content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    essence.starts_with("text/")
        || essence.ends_with("+json")
        || essence.ends_with("+xml")
        || matches!(
            essence.as_str(),
            "application/json"
                | "application/javascript"
                | "application/x-javascript"
                | "application/ecmascript"
                | "application/xml"
        )
}

fn url_for(target: &LoopbackTarget, req: &TunnelRequest) -> String {
    let host = if target.host.contains(':') && !target.host.starts_with('[') {
        format!("[{}]", target.host)
    } else {
        target.host.clone()
    };
    let mut url = format!("http://{host}:{}{}", target.port, req.path);
    for (i, (k, v)) in req.query.iter().enumerate() {
        url.push(if i == 0 { '?' } else { '&' });
        url.push_str(&utf8_percent_encode(k, QUERY_VALUE).to_string());
        url.push('=');
        url.push_str(&utf8_percent_encode(v, QUERY_VALUE).to_string());
    }
    url
}

/// Make the loopback call and shape its answer.
///
/// Runs on its own thread: `reqwest::blocking` drives a private tokio runtime,
/// and the relay hands handlers a `spawn_blocking` thread that is still inside
/// the WebSocket client's runtime context.
fn forward(target: LoopbackTarget, req: TunnelRequest) -> Value {
    let handle = std::thread::spawn(move || forward_blocking(&target, req));
    handle
        .join()
        .unwrap_or_else(|_| error_reply(502, "loopback call panicked"))
}

fn forward_blocking(target: &LoopbackTarget, req: TunnelRequest) -> Value {
    let client = match reqwest::blocking::Client::builder()
        .timeout(LOOPBACK_TIMEOUT)
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
    {
        Ok(c) => c,
        Err(e) => return error_reply(502, &format!("loopback client: {e}")),
    };
    let method = match reqwest::Method::from_bytes(req.method.as_bytes()) {
        Ok(m) => m,
        Err(_) => return error_reply(400, "invalid method"),
    };
    let mut builder = client.request(method, url_for(target, &req));
    if let Some(t) = target.token.as_deref() {
        builder = builder.bearer_auth(t);
    }
    if let Some(ct) = req.content_type.as_deref() {
        builder = builder.header(reqwest::header::CONTENT_TYPE, ct);
    }
    if let Some(body) = req.body {
        builder = builder.body(body);
    }
    let resp = match builder.send() {
        Ok(r) => r,
        Err(e) => return error_reply(502, &format!("loopback call failed: {e}")),
    };
    let status = resp.status().as_u16();
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();
    if resp
        .content_length()
        .is_some_and(|n| n > MAX_RESPONSE_BYTES as u64)
    {
        return too_large();
    }
    // Read one byte past the cap so an unsized (chunked) body over it is
    // caught without buffering the whole thing.
    let mut bytes = Vec::new();
    if let Err(e) = std::io::Read::read_to_end(
        &mut std::io::Read::take(resp, MAX_RESPONSE_BYTES as u64 + 1),
        &mut bytes,
    ) {
        return error_reply(502, &format!("loopback read failed: {e}"));
    }
    if bytes.len() > MAX_RESPONSE_BYTES {
        return too_large();
    }
    encode_reply(status, &content_type, bytes)
}

fn too_large() -> Value {
    error_reply(
        413,
        &format!("response body over the {MAX_RESPONSE_BYTES}-byte relay tunnel limit"),
    )
}

fn encode_reply(status: u16, content_type: &str, bytes: Vec<u8>) -> Value {
    if bytes.is_empty() {
        return json!({
            "status": status, "contentType": content_type,
            "body": Value::Null, "bodyB64": Value::Null,
        });
    }
    if is_textual(content_type) {
        match String::from_utf8(bytes) {
            Ok(text) => {
                return json!({
                    "status": status, "contentType": content_type,
                    "body": text, "bodyB64": Value::Null,
                })
            }
            Err(e) => {
                let bytes = e.into_bytes();
                return json!({
                    "status": status, "contentType": content_type,
                    "body": Value::Null,
                    "bodyB64": base64::engine::general_purpose::STANDARD.encode(bytes),
                });
            }
        }
    }
    json!({
        "status": status, "contentType": content_type,
        "body": Value::Null,
        "bodyB64": base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

/// Answer `GET /sessions_snapshot` from the process's registered session
/// tracker (the desktop app's), when there is one — see
/// `relay_events::set_sessions_snapshot_provider`. Same contract as the route:
/// the full frame, or `204` when `since` already names its seq. `None` means
/// no provider, and the request goes to the loopback server as usual.
fn local_sessions_snapshot_reply(req: &TunnelRequest) -> Option<Value> {
    if req.method != "GET" || req.path != crate::routes::SESSIONS_SNAPSHOT {
        return None;
    }
    let frame = crate::relay_events::local_sessions_snapshot()?;
    let since = req
        .query
        .iter()
        .rev()
        .find(|(k, _)| k == "since")
        .and_then(|(_, v)| v.parse::<u64>().ok());
    if since == Some(frame.seq()) {
        return Some(encode_reply(204, "application/json", Vec::new()));
    }
    let body = serde_json::to_vec(&frame).unwrap_or_default();
    Some(encode_reply(200, "application/json", body))
}

/// The `http` relay method.
pub fn serve_http(params: &Value) -> Result<Value, String> {
    Ok(serve_http_via(params, resolve_target))
}

fn serve_http_via(
    params: &Value,
    target: impl FnOnce() -> Result<LoopbackTarget, String>,
) -> Value {
    let req = match parse(params) {
        Ok(r) => r,
        Err(reply) => return reply,
    };
    if !is_allowed(&req.method, &req.path) {
        return error_reply(
            403,
            &format!(
                "{} {} is not available over the relay",
                req.method, req.path
            ),
        );
    }
    if let Some(reply) = local_sessions_snapshot_reply(&req) {
        return reply;
    }
    match target() {
        Ok(t) => forward(t, req),
        Err(e) => error_reply(502, &format!("no local server: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// What the fake loopback server saw.
    #[derive(Debug)]
    struct Seen {
        method: String,
        url: String,
        auth: Option<String>,
        content_type: Option<String>,
        body: Vec<u8>,
    }

    /// One-shot tiny_http server answering `(status, content_type, body)`.
    fn fake_server(
        status: u16,
        content_type: &'static str,
        body: Vec<u8>,
    ) -> (LoopbackTarget, mpsc::Receiver<Seen>) {
        let server = tiny_http::Server::http("127.0.0.1:0").unwrap();
        let port = server.server_addr().to_ip().unwrap().port();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            if let Ok(mut req) = server.recv() {
                fn header(req: &tiny_http::Request, name: &'static str) -> Option<String> {
                    req.headers()
                        .iter()
                        .find(|h| h.field.equiv(name))
                        .map(|h| h.value.as_str().to_string())
                }
                let auth = header(&req, "authorization");
                let ct = header(&req, "content-type");
                let mut got = Vec::new();
                let _ = std::io::Read::read_to_end(req.as_reader(), &mut got);
                let _ = tx.send(Seen {
                    method: req.method().to_string(),
                    url: req.url().to_string(),
                    auth,
                    content_type: ct,
                    body: got,
                });
                let res = tiny_http::Response::from_data(body)
                    .with_status_code(status)
                    .with_header(
                        format!("Content-Type: {content_type}")
                            .parse::<tiny_http::Header>()
                            .unwrap(),
                    );
                let _ = req.respond(res);
            }
        });
        (
            LoopbackTarget {
                host: "127.0.0.1".into(),
                port,
                token: Some("tok".into()),
            },
            rx,
        )
    }

    #[test]
    fn allowed_get_is_forwarded_with_token_and_encoded_query() {
        let (target, rx) = fake_server(200, "application/json", br#"[{"id":"s1"}]"#.to_vec());
        let reply = serve_http_via(
            &json!({"method": "GET", "path": "/sessions",
                    "query": {"path": "/a b/c", "n": 3, "skip": null}}),
            || Ok(target),
        );
        let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(seen.method, "GET");
        assert!(seen.url.starts_with("/sessions?"), "url: {}", seen.url);
        assert!(seen.url.contains("path=%2Fa%20b%2Fc"), "url: {}", seen.url);
        assert!(seen.url.contains("n=3"), "url: {}", seen.url);
        assert!(!seen.url.contains("skip"), "null query values are dropped");
        assert_eq!(seen.auth.as_deref(), Some("Bearer tok"));
        assert_eq!(reply["status"], 200);
        assert_eq!(reply["contentType"], "application/json");
        assert_eq!(reply["body"], r#"[{"id":"s1"}]"#);
        assert!(reply["bodyB64"].is_null());
    }

    #[test]
    fn allowed_post_forwards_json_body() {
        let (target, rx) = fake_server(200, "application/json", b"{}".to_vec());
        let reply = serve_http_via(
            &json!({"method": "POST", "path": "/fleet-ask/respond",
                    "body": {"id": "r1", "answers": {"q": "yes"}}}),
            || Ok(target),
        );
        let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(seen.method, "POST");
        assert_eq!(seen.content_type.as_deref(), Some("application/json"));
        let sent: Value = serde_json::from_slice(&seen.body).unwrap();
        assert_eq!(sent["answers"]["q"], "yes");
        assert_eq!(reply["status"], 200);
    }

    #[test]
    fn denied_routes_get_403_without_a_loopback_call() {
        for (method, path) in [
            ("POST", "/proc_run"),
            ("POST", "/spawn_session"),
            ("GET", "/stop"),
            ("GET", "/interrupt"),
            ("POST", "/apply_guard_hook"),
            ("POST", "/mobile_rpc"),
            ("GET", "/events"),
            ("POST", "/sessions"),
            ("GET", "/explorer_file"),
            ("GET", "/daily_report/generate"),
        ] {
            let reply = serve_http_via(&json!({"method": method, "path": path}), || {
                panic!("{method} {path} must not reach the loopback target")
            });
            assert_eq!(reply["status"], 403, "{method} {path}");
            assert!(reply["body"].as_str().unwrap().contains("not available"));
        }
    }

    #[test]
    fn traversal_and_smuggled_paths_are_rejected_before_matching() {
        for path in [
            "/wiki_asset/../proc_run",
            "/wiki_asset/a/%2e%2e/b",
            "/decision_asset/a%2Fb",
            "/wiki_asset/p%2Ff%2Fdoc/v1/a%2F..%2Fb",
            "/wiki_asset/p%2Ff%2Fdoc/v1%2Fx/index.html",
            "/wiki_asset/p%2F%2e%2e/v1/index.html",
            "/wiki_asset/p%2Ff%2Fdoc",
            "/sessions%2Fx",
            "/sessions?x=1",
            "sessions",
            "/wiki_asset/",
        ] {
            let reply = serve_http_via(&json!({"method": "GET", "path": path}), || {
                panic!("{path} must not reach the loopback target")
            });
            let status = reply["status"].as_u64().unwrap();
            assert!(status == 400 || status == 403, "{path}: {status}");
        }
    }

    #[test]
    fn binary_response_comes_back_as_base64() {
        let png = vec![0x89, b'P', b'N', b'G', 0, 0xff, 0x10];
        let (target, _rx) = fake_server(200, "image/png", png.clone());
        let reply = serve_http_via(
            &json!({"method": "GET", "path": "/decision_asset/abc/img.png"}),
            || Ok(target),
        );
        assert_eq!(reply["status"], 200);
        assert_eq!(reply["contentType"], "image/png");
        assert!(reply["body"].is_null());
        let back = base64::engine::general_purpose::STANDARD
            .decode(reply["bodyB64"].as_str().unwrap())
            .unwrap();
        assert_eq!(back, png);
    }

    #[test]
    fn wiki_asset_slug_keeps_its_encoded_slashes() {
        // Wiki slugs are `<project>/<folder>/<name>`; `wikiFileUrl` encodes
        // them into one segment. The path must reach the server verbatim.
        let (target, rx) = fake_server(200, "text/html", b"<p>ok</p>".to_vec());
        let path = "/wiki_asset/claude-fleet%2Fe2e%2Fwikidoc/20261110-164348/index.html";
        let reply = serve_http_via(&json!({"method": "GET", "path": path}), || Ok(target));
        assert_eq!(reply["status"], 200, "reply: {reply}");
        assert_eq!(rx.recv().unwrap().url, path);
    }

    #[test]
    fn binary_upload_is_decoded_from_body_b64() {
        let (target, rx) = fake_server(200, "application/json", b"{}".to_vec());
        // Not a whitelisted upload route today; exercise the decode via an
        // allowed POST so the wire shape is covered.
        let raw = vec![0u8, 1, 2, 250];
        let _ = serve_http_via(
            &json!({"method": "POST", "path": "/guard/respond",
                    "bodyB64": base64::engine::general_purpose::STANDARD.encode(&raw),
                    "contentType": "application/octet-stream"}),
            || Ok(target),
        );
        let seen = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(seen.body, raw);
        assert_eq!(
            seen.content_type.as_deref(),
            Some("application/octet-stream")
        );
    }

    #[test]
    fn oversized_response_is_413() {
        let big = vec![b'x'; MAX_RESPONSE_BYTES + 1];
        let (target, _rx) = fake_server(200, "application/octet-stream", big);
        let reply = serve_http_via(
            &json!({"method": "GET", "path": "/wiki_asset/big.bin"}),
            || Ok(target),
        );
        assert_eq!(reply["status"], 413);
        assert!(reply["body"].as_str().unwrap().contains("16777216"));
    }

    #[test]
    fn both_bodies_is_a_400() {
        let reply = serve_http_via(
            &json!({"method": "POST", "path": "/guard/respond", "body": {}, "bodyB64": "AA=="}),
            || panic!("must not forward"),
        );
        assert_eq!(reply["status"], 400);
    }

    #[test]
    fn unreachable_target_is_502() {
        // Bind then drop to get a port nothing listens on.
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let reply = serve_http_via(&json!({"method": "GET", "path": "/health"}), || {
            Ok(LoopbackTarget {
                host: "127.0.0.1".into(),
                port,
                token: None,
            })
        });
        assert_eq!(reply["status"], 502);
    }

    /// With a registered tracker (the desktop app's), `/sessions_snapshot` is
    /// answered from it — never the loopback server, whose tracker numbers a
    /// different seq space — with the route's `since` → 204 contract.
    #[test]
    fn sessions_snapshot_comes_from_the_registered_tracker() {
        crate::relay_events::set_sessions_snapshot_provider(|| {
            crate::session_delta::SessionsFrame::Full {
                seq: 7,
                sessions: vec![json!({"id": "s1"})],
            }
        });
        let no_loopback = || -> Result<LoopbackTarget, String> {
            panic!("must not reach the loopback server")
        };
        let reply = serve_http_via(
            &json!({"method": "GET", "path": "/sessions_snapshot"}),
            no_loopback,
        );
        assert_eq!(reply["status"], 200);
        let frame: Value = serde_json::from_str(reply["body"].as_str().unwrap()).unwrap();
        assert_eq!(frame["kind"], "full");
        assert_eq!(frame["seq"], 7);
        let reply = serve_http_via(
            &json!({"method": "GET", "path": "/sessions_snapshot", "query": {"since": "7"}}),
            no_loopback,
        );
        assert_eq!(reply["status"], 204);
        assert!(reply["body"].is_null());
        let reply = serve_http_via(
            &json!({"method": "GET", "path": "/sessions_snapshot", "query": {"since": 6}}),
            no_loopback,
        );
        assert_eq!(reply["status"], 200);
    }

    #[test]
    fn textual_types() {
        assert!(is_textual("application/json"));
        assert!(is_textual("text/html; charset=utf-8"));
        assert!(is_textual("image/svg+xml"));
        assert!(is_textual("application/javascript"));
        assert!(!is_textual("image/png"));
        assert!(!is_textual("application/pdf"));
    }

    #[test]
    fn wildcard_bind_hosts_connect_over_loopback() {
        assert_eq!(connect_host("0.0.0.0"), "127.0.0.1");
        assert_eq!(connect_host("::"), "127.0.0.1");
        assert_eq!(connect_host("192.168.1.5"), "192.168.1.5");
    }

    #[test]
    fn mid_segment_wildcard_matches_one_segment() {
        assert!(is_allowed("GET", "/sources/codex/usage"));
        assert!(is_allowed("GET", "/sources/claude/account"));
        assert!(!is_allowed("GET", "/sources/codex/account"));
        assert!(!is_allowed("GET", "/sources//usage"));
        assert!(!is_allowed("GET", "/sources/a/b/usage"));
        assert!(!is_allowed("GET", "/sources/codex/usage/x"));
        assert!(is_allowed("GET", "/decision_asset/abc/0/chart.png"));
        assert!(!is_allowed("GET", "/decision_asset/"));
        assert!(!is_allowed("GET", "/remote_workspaces"));
        assert!(!is_allowed("GET", "/ssh_hosts"));
        assert!(!is_allowed("GET", "/audit"));
        assert!(is_allowed("POST", "/elicitation/upload"));
    }

    /// Every whitelisted exact path must be a route `fleet serve` actually
    /// dispatches — a typo here would silently 404 instead of failing review.
    #[test]
    fn whitelisted_paths_are_known_routes() {
        let src = include_str!("routes.rs");
        for (_, p) in HTTP_TUNNEL_ROUTES {
            let needle = &p[..p.find('*').unwrap_or(p.len())];
            assert!(
                src.contains(&format!("\"{needle}\"")),
                "{p} is not a path constant in routes.rs"
            );
        }
    }
}
