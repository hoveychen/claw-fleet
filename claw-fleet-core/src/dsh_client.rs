//! RPC client for the dsh (DeepSeek Harness) `/api` face.
//!
//! `dsh web` serves one HTTP route (`/api`) plus a downlink WebSocket. Every
//! unary call is a POST to `/api/<endpoint>` carrying a `client-request`
//! envelope; the HTTP response body is the matching `server-response`:
//!
//! ```text
//! POST /api/session/list
//! {"type":"client-request","rpcId":"<uuid>","method":"session/list",
//!  "payload":{"args":{"_request":{}}}}
//! → {"type":"server-response","rpcId":"<same uuid>","result":{"ok":true,"value":{…}}}
//! ```
//!
//! Endpoints are `<service>/<method>` — exactly two slash-separated segments,
//! which the gateway enforces. dsh 0.1.1 spelled them `service.method`; the
//! dotted form is a 404 on 0.1.2 because it parses as one segment.
//!
//! The `payload` is always `{"args": …}` (see [`build_request`]), and the
//! argument object's own field name is the endpoint's business: most session
//! calls take `request`, `session/list` takes `_request`, `credentials/*` take
//! `ref`/`refs`, `agentPresets/read` takes `agentPreset`.
//!
//! Answerable downlink frames (`approval/requested`, `question/requested`) are
//! answered on a *different* carrier: POST `/api/respond` with a
//! `client-response` echoing the frame's `rpcId`, whose body is a carrier
//! receipt rather than a `server-response`.
//!
//! `/api` sits behind two gates, in this order:
//!
//! 1. **Host-header loopback fence** — a non-loopback `Host` is answered 403
//!    before dispatch, so every call here targets `127.0.0.1`.
//! 2. **Browser authentication** — an unauthenticated request is answered 401.
//!    `dsh web` mints one random launch token per process and prints it as the
//!    query of the URL it announces (`http://127.0.0.1:<port>/?token=<token>`);
//!    that stdout line is the token's only exit from the process. `GET
//!    /?token=…` answers 303 with a `Set-Cookie` bound to the request's
//!    authority, and `/api` accepts that cookie — and *only* that cookie. It
//!    does not read `?token=` itself, so [`DshClient::new`] performs the
//!    exchange once at construction and replays the cookie on every request.

use std::time::Duration;

use serde_json::{json, Value};

use crate::off_runtime::off_runtime;

/// Default per-call timeout. `session/prompt` returns as soon as the turn is
/// admitted (not when it finishes), so no call on this face is long-polling.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// A failed `/api` call, split by which layer rejected it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DshRpcError {
    /// The carrier failed: connection refused, timeout, or a non-2xx status
    /// (403 = the Host-header trust fence rejected us before dispatch).
    Transport(String),
    /// The carrier succeeded but the envelope was unusable: unparseable JSON,
    /// wrong `type`, or an `rpcId` that does not echo what we sent.
    Envelope(String),
    /// The business layer answered `{"ok":false,…}`. `code` is dsh's stable
    /// error taxonomy (`bad-request`, `agent-busy`, `method-unavailable`, …).
    Rpc { code: String, message: String },
    /// `/api/respond` accepted the carrier but refused the answer: the frame
    /// was already settled (`not-pending`) or the payload had the wrong shape
    /// (`bad-response` — e.g. an approval answer missing `approvalId`).
    Rejected(String),
}

impl std::fmt::Display for DshRpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Transport(m) => write!(f, "dsh transport: {m}"),
            Self::Envelope(m) => write!(f, "dsh envelope: {m}"),
            Self::Rpc { code, message } => write!(f, "dsh rpc {code}: {message}"),
            Self::Rejected(reason) => write!(f, "dsh respond rejected: {reason}"),
        }
    }
}

impl std::error::Error for DshRpcError {}

impl From<DshRpcError> for String {
    fn from(e: DshRpcError) -> Self {
        e.to_string()
    }
}

/// Build the `client-request` envelope body for one unary call.
///
/// Returns the minted `rpcId` alongside the serialized body so the caller can
/// verify the echo. Correlation is per-call: dsh rejects a `server-response`
/// whose id does not match, and so do we.
///
/// `args` is the endpoint's own argument object, which this wraps in the
/// gateway's `{"args": …}` envelope. 0.1.2 checks that wrapper before it ever
/// looks at the endpoint — a payload holding the arguments directly is
/// answered `gateway/internal: Remote payload must contain exactly one
/// plain-object args field`, whatever the method was. Wrapping here rather
/// than at each call site keeps the *inner* field names (which differ per
/// endpoint: `request`, `_request`, `refs`, `agentPreset`, …) the call site's
/// business and the envelope this module's.
fn build_request(rpc_id: &str, method: &str, args: &Value) -> String {
    json!({
        "type": "client-request",
        "rpcId": rpc_id,
        "method": method,
        "payload": { "args": args },
    })
    .to_string()
}

/// Decode a `server-response` body against the id we sent.
///
/// Split out from the HTTP call so the envelope contract is unit-testable
/// without a live server.
fn parse_response(sent_rpc_id: &str, body: &str) -> Result<Value, DshRpcError> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|e| DshRpcError::Envelope(format!("malformed json: {e}")))?;

    match parsed.get("type").and_then(Value::as_str) {
        Some("server-response") => {}
        other => {
            return Err(DshRpcError::Envelope(format!(
                "expected server-response, got {}",
                other.unwrap_or("<missing>")
            )))
        }
    }

    let echoed = parsed.get("rpcId").and_then(Value::as_str).unwrap_or("");
    if echoed != sent_rpc_id {
        return Err(DshRpcError::Envelope(format!(
            "rpcId mismatch: sent {sent_rpc_id}, got {echoed}"
        )));
    }

    let result = parsed
        .get("result")
        .ok_or_else(|| DshRpcError::Envelope("response has no result".into()))?;

    if result.get("ok").and_then(Value::as_bool) == Some(true) {
        // An `ok` result with no `value` is a unit return (`credentials/set`
        // answers `{"ok":true,"value":{}}`, but the slot may be elided).
        return Ok(result.get("value").cloned().unwrap_or_else(|| json!({})));
    }

    let error = result.get("error");
    Err(DshRpcError::Rpc {
        code: error
            .and_then(|e| e.get("code"))
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
        message: error
            .and_then(|e| e.get("message"))
            .and_then(Value::as_str)
            .unwrap_or("no message")
            .to_string(),
    })
}

/// Build the `client-response` envelope body for one answered downlink frame.
///
/// The `rpcId` is the *frame's*, not one we mint: `/api/respond` routes the
/// answer through its pending table by that id, so an id of our own would come
/// back `not-pending`.
fn build_client_response(rpc_id: &str, result: Value) -> String {
    json!({
        "type": "client-response",
        "rpcId": rpc_id,
        "result": result,
    })
    .to_string()
}

/// Decode the carrier receipt returned by `/api/respond`.
///
/// This is deliberately not an `RpcResult`: dsh models it as a carrier-layer
/// receipt, so a refused answer is `{"accepted":false,"reason":…}` with HTTP
/// 200 — treating a 200 as success would silently drop the refusal.
fn parse_receipt(body: &str) -> Result<(), DshRpcError> {
    let parsed: Value = serde_json::from_str(body)
        .map_err(|e| DshRpcError::Envelope(format!("malformed receipt json: {e}")))?;

    if parsed.get("accepted").and_then(Value::as_bool) == Some(true) {
        return Ok(());
    }

    Err(DshRpcError::Rejected(
        parsed
            .get("reason")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string(),
    ))
}

/// The `/api` base for one loopback port.
fn api_base(port: u16) -> String {
    format!("http://127.0.0.1:{port}/api")
}

/// Pull the `name=value` pair out of one `Set-Cookie` header.
///
/// Split out from the exchange so the parsing is testable without a live
/// server: everything from the first `;` on is attributes (`Path`, `Max-Age`,
/// `HttpOnly`, …) that a request must not echo back.
fn cookie_pair(set_cookie: &str) -> Option<String> {
    let pair = set_cookie.split(';').next()?.trim();
    (pair.contains('=') && !pair.starts_with('=')).then(|| pair.to_string())
}

/// Blocking client for one `dsh web` instance on loopback.
pub struct DshClient {
    base: String,
    http: reqwest::blocking::Client,
    /// The `name=value` minted by the launch-token exchange, replayed on every
    /// request. Without it `/api` answers 401.
    cookie: String,
}

impl DshClient {
    /// Build a client for `127.0.0.1:<port>`, trading `launch_token` for the
    /// session cookie `/api` requires.
    ///
    /// The host is fixed: dsh's `/api` fence only admits loopback authorities
    /// (or an explicitly declared `--trusted-host`), and Fleet always runs the
    /// server it talks to on the same machine. That matters twice over here —
    /// the cookie dsh mints is bound to the authority that asked for it, so it
    /// is only valid for requests carrying this same `127.0.0.1:<port>` Host.
    pub fn new(port: u16, launch_token: &str) -> Result<Self, DshRpcError> {
        let http = off_runtime(|| {
            reqwest::blocking::Client::builder()
                .timeout(DEFAULT_TIMEOUT)
                // The exchange answers 303 → `/`. Following it would drop the
                // `Set-Cookie` we came for on the floor and return the index
                // page instead, so read the redirect rather than chase it.
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .map_err(|e| DshRpcError::Transport(format!("http client: {e}")))
        })
        .map_err(DshRpcError::Transport)??;

        let cookie = Self::exchange_token(&http, port, launch_token)?;
        Ok(Self {
            base: api_base(port),
            http,
            cookie,
        })
    }

    /// Trade the launch token for an authority-bound session cookie.
    fn exchange_token(
        http: &reqwest::blocking::Client,
        port: u16,
        launch_token: &str,
    ) -> Result<String, DshRpcError> {
        let url = format!("http://127.0.0.1:{port}/?token={launch_token}");
        let header = off_runtime(|| {
            let resp = http
                .get(&url)
                .send()
                .map_err(|e| DshRpcError::Transport(format!("token exchange: {e}")))?;
            let status = resp.status();
            let set_cookie = resp
                .headers()
                .get(reqwest::header::SET_COOKIE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string);
            Ok((status, set_cookie))
        })
        .map_err(DshRpcError::Transport)??;

        let (status, set_cookie) = header;
        match set_cookie.as_deref().and_then(cookie_pair) {
            Some(pair) => Ok(pair),
            // A stale or wrong token is answered 401 with no cookie; anything
            // else means dsh changed the exchange out from under us. Both are
            // worth naming, because every later call would just say 401.
            None => Err(DshRpcError::Transport(format!(
                "token exchange: HTTP {status} with no usable Set-Cookie \
                 (is this dsh older than 0.1.2, or the token stale?)"
            ))),
        }
    }

    /// The `/api` base URL this client targets.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Invoke one unary method and return its `result.value`.
    pub fn call(&self, method: &str, payload: Value) -> Result<Value, DshRpcError> {
        let rpc_id = uuid::Uuid::new_v4().to_string();
        let body = build_request(&rpc_id, method, &payload);

        let (status, text) = off_runtime(|| {
            let resp = self
                .http
                .post(format!("{}/{}", self.base, method))
                .header("content-type", "application/json")
                .header(reqwest::header::COOKIE, &self.cookie)
                .body(body)
                .send()
                .map_err(|e| DshRpcError::Transport(format!("{method}: {e}")))?;

            let status = resp.status();
            let text = resp
                .text()
                .map_err(|e| DshRpcError::Transport(format!("{method} body: {e}")))?;
            Ok((status, text))
        })
        .map_err(DshRpcError::Transport)??;

        if !status.is_success() {
            return Err(DshRpcError::Transport(format!(
                "{method}: HTTP {status}{}",
                match status.as_u16() {
                    403 => " (loopback trust fence)",
                    401 => " (session cookie rejected)",
                    // A live `/api` answers an unknown endpoint with 404 rather
                    // than an envelope error, and the dotted 0.1.1 spelling is
                    // exactly that shape — so name the likely cause.
                    404 => " (no such endpoint — endpoints are <service>/<method>)",
                    _ => "",
                }
            )));
        }

        parse_response(&rpc_id, &text)
    }

    /// Answer an answerable downlink frame, echoing its `rpcId`.
    ///
    /// `value` is the frame domain's response payload — for an approval that is
    /// `{sessionId, approvalId, outcome}`; omitting `approvalId` is refused
    /// with `bad-response`.
    pub fn respond(&self, rpc_id: &str, value: Value) -> Result<(), DshRpcError> {
        self.post_response(
            rpc_id,
            json!({ "ok": true, "value": value }),
        )
    }

    /// Withdraw an answerable frame instead of answering it.
    ///
    /// The only refusal dsh accepts: `/api/respond` maps an `ok:false` result
    /// to a cancellation *only* when the error code is `cancelled` (every other
    /// code is refused as `bad-response`), and only questions are cancellable —
    /// an approval expects one of its two outcomes instead. `details` must be
    /// present and empty: the error schema is a discriminated union whose
    /// `cancelled` arm declares `details: {}`.
    pub fn respond_cancelled(&self, rpc_id: &str, message: &str) -> Result<(), DshRpcError> {
        self.post_response(
            rpc_id,
            json!({
                "ok": false,
                "error": { "code": "cancelled", "message": message, "details": {} },
            }),
        )
    }

    /// POST one `client-response` envelope and decode its carrier receipt.
    fn post_response(&self, rpc_id: &str, result: Value) -> Result<(), DshRpcError> {
        let body = build_client_response(rpc_id, result);

        let (status, text) = off_runtime(|| {
            let resp = self
                .http
                .post(format!("{}/respond", self.base))
                .header("content-type", "application/json")
                .header(reqwest::header::COOKIE, &self.cookie)
                .body(body)
                .send()
                .map_err(|e| DshRpcError::Transport(format!("respond: {e}")))?;

            let status = resp.status();
            let text = resp
                .text()
                .map_err(|e| DshRpcError::Transport(format!("respond body: {e}")))?;
            Ok((status, text))
        })
        .map_err(DshRpcError::Transport)??;

        if !status.is_success() {
            return Err(DshRpcError::Transport(format!("respond: HTTP {status}")));
        }

        parse_receipt(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// tauri `(async)` commands run their sync bodies on tokio runtime workers,
    /// and that is where every UI-triggered dsh RPC executes. reqwest's blocking
    /// carrier refuses that context (`wait::enter` → "Cannot drop a runtime…"),
    /// and the panic is swallowed by the task harness, so the invoke promise
    /// hangs forever — the 「永久加载中」 bug. Construct must therefore survive
    /// inside a tokio worker: a clean transport Err (nothing listens on the
    /// probed port), never a panic. Construction is the sharper end of that
    /// contract since 0.1.2, because it now issues the token exchange itself —
    /// the very first blocking request Fleet makes.
    #[test]
    fn client_survives_tokio_worker_context() {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();
        let joined = rt.block_on(async {
            tokio::spawn(async {
                let client = DshClient::new(1, "not-a-real-token").map_err(|e| e.to_string())?;
                client
                    .call("session/list", json!({}))
                    .map(|_| ())
                    .map_err(|e| e.to_string())
            })
            .await
        });
        let inner = joined.expect("dsh client must not panic inside a tokio worker");
        // Port 1 has no listener: the healthy outcome is a transport error.
        assert!(inner.is_err());
    }

    #[test]
    fn build_request_carries_method_and_id() {
        let body = build_request("id-1", "session/list", &json!({}));
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["type"], "client-request");
        assert_eq!(parsed["rpcId"], "id-1");
        assert_eq!(parsed["method"], "session/list");
        assert_eq!(parsed["payload"], json!({ "args": {} }));
    }

    /// The 0.1.2 gateway rejects a payload that is not exactly `{args: {…}}`
    /// with `gateway/internal: Remote payload must contain exactly one
    /// plain-object args field` — verbatim from a live `settings/describe`
    /// probe on 0.1.2-rc.1. Call sites keep passing the bare argument object
    /// (whose field names are per-endpoint: `request`, `_request`, `refs`,
    /// `agentPreset`, …), so the wrapper belongs here, once.
    #[test]
    fn build_request_wraps_the_arguments_in_the_gateway_args_field() {
        let body = build_request("id-2", "session/page", &json!({ "request": { "id": "s1" } }));
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["payload"], json!({ "args": { "request": { "id": "s1" } } }));
    }

    #[test]
    fn parse_response_returns_ok_value() {
        // Shape taken verbatim from a live `session/create` response.
        let body = r#"{"type":"server-response","rpcId":"id-1",
            "result":{"ok":true,"value":{"sessionId":"session-abc","agentPreset":"standard"}}}"#;
        let value = parse_response("id-1", body).unwrap();
        assert_eq!(value["sessionId"], "session-abc");
    }

    #[test]
    fn parse_response_defaults_missing_value_to_empty_object() {
        let body = r#"{"type":"server-response","rpcId":"id-1","result":{"ok":true}}"#;
        assert_eq!(parse_response("id-1", body).unwrap(), json!({}));
    }

    #[test]
    fn parse_response_surfaces_business_error() {
        // Shape taken verbatim from a live `credentials/describe` rejection.
        let body = r#"{"type":"server-response","rpcId":"id-1","result":{"ok":false,
            "error":{"code":"bad-request","message":"invalid payload for credentials/describe"}}}"#;
        match parse_response("id-1", body).unwrap_err() {
            DshRpcError::Rpc { code, message } => {
                assert_eq!(code, "bad-request");
                assert!(message.contains("invalid payload"));
            }
            other => panic!("expected Rpc error, got {other:?}"),
        }
    }

    #[test]
    fn parse_response_rejects_rpc_id_mismatch() {
        let body = r#"{"type":"server-response","rpcId":"other","result":{"ok":true,"value":{}}}"#;
        assert!(matches!(
            parse_response("id-1", body).unwrap_err(),
            DshRpcError::Envelope(_)
        ));
    }

    #[test]
    fn parse_response_rejects_wrong_envelope_type() {
        // A downlink frame must never be read as a unary answer.
        let body = r#"{"type":"server-request","rpcId":"id-1","method":"approval/requested","payload":{}}"#;
        assert!(matches!(
            parse_response("id-1", body).unwrap_err(),
            DshRpcError::Envelope(_)
        ));
    }

    #[test]
    fn parse_response_rejects_malformed_json() {
        assert!(matches!(
            parse_response("id-1", "not json").unwrap_err(),
            DshRpcError::Envelope(_)
        ));
    }

    #[test]
    fn parse_receipt_accepts_true() {
        assert!(parse_receipt(r#"{"accepted":true}"#).is_ok());
    }

    #[test]
    fn parse_receipt_surfaces_bad_response() {
        // Observed live when an approval answer omitted `approvalId`.
        match parse_receipt(r#"{"accepted":false,"reason":"bad-response"}"#).unwrap_err() {
            DshRpcError::Rejected(reason) => assert_eq!(reason, "bad-response"),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn parse_receipt_surfaces_not_pending() {
        match parse_receipt(r#"{"accepted":false,"reason":"not-pending"}"#).unwrap_err() {
            DshRpcError::Rejected(reason) => assert_eq!(reason, "not-pending"),
            other => panic!("expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn client_response_echoes_the_frames_rpc_id() {
        let body = build_client_response("frame-1", json!({ "ok": true, "value": { "a": 1 } }));
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["type"], "client-response");
        assert_eq!(parsed["rpcId"], "frame-1");
        assert_eq!(parsed["result"]["value"]["a"], 1);
    }

    /// dsh's error schema is a discriminated union: the `cancelled` arm declares
    /// `details: {}`, so an envelope that omits the slot fails validation and
    /// comes back `bad-response` instead of withdrawing the question.
    #[test]
    fn a_cancellation_carries_the_cancelled_code_and_an_empty_details() {
        let body = build_client_response(
            "frame-1",
            json!({ "ok": false, "error": { "code": "cancelled", "message": "m", "details": {} } }),
        );
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["result"]["ok"], false);
        assert_eq!(parsed["result"]["error"]["code"], "cancelled");
        assert_eq!(parsed["result"]["error"]["details"], json!({}));
    }

    #[test]
    fn client_targets_loopback() {
        assert_eq!(api_base(3080), "http://127.0.0.1:3080/api");
    }

    #[test]
    fn cookie_pair_keeps_only_the_name_value() {
        // Shape taken from a live `dsh web` 0.1.2-rc.1 exchange.
        let raw = "dsh-auth-q76Y4r_EfF9y=v1.eyJ2IjoxfQ.sig; Max-Age=2592000; \
                   Path=/; Expires=Mon, 05 Oct 2026 17:34:29 GMT; HttpOnly; SameSite=Strict";
        assert_eq!(
            cookie_pair(raw).as_deref(),
            Some("dsh-auth-q76Y4r_EfF9y=v1.eyJ2IjoxfQ.sig")
        );
    }

    #[test]
    fn cookie_pair_rejects_headers_without_a_pair() {
        assert_eq!(cookie_pair(""), None);
        assert_eq!(cookie_pair("Path=/; HttpOnly").as_deref(), Some("Path=/"));
        assert_eq!(cookie_pair("=orphan; Path=/"), None);
        assert_eq!(cookie_pair("justaname; Path=/"), None);
    }
}
