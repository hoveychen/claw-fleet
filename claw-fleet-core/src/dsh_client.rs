//! RPC client for the dsh (DeepSeek Harness) `/api` face.
//!
//! `dsh web` serves one HTTP route (`/api`) plus two downlink-only WebSockets.
//! Every unary call is a POST to `/api/<method>` carrying a `client-request`
//! envelope; the HTTP response body is the matching `server-response`:
//!
//! ```text
//! POST /api/session.list
//! {"type":"client-request","rpcId":"<uuid>","method":"session.list","payload":{}}
//! → {"type":"server-response","rpcId":"<same uuid>","result":{"ok":true,"value":{…}}}
//! ```
//!
//! Answerable downlink frames (`approval/requested`, `question/requested`) are
//! answered on a *different* carrier: POST `/api/respond` with a
//! `client-response` echoing the frame's `rpcId`, whose body is a carrier
//! receipt rather than a `server-response`.
//!
//! The server has no authentication layer. It guards `/api` with a Host-header
//! loopback fence only, so every call here targets `127.0.0.1` — a non-loopback
//! `Host` is answered 403 before dispatch.

use std::time::Duration;

use serde_json::{json, Value};

/// Default per-call timeout. `session.prompt` returns as soon as the turn is
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
fn build_request(rpc_id: &str, method: &str, payload: &Value) -> String {
    json!({
        "type": "client-request",
        "rpcId": rpc_id,
        "method": method,
        "payload": payload,
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
        // An `ok` result with no `value` is a unit return (`credentials.set`
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

/// Blocking client for one `dsh web` instance on loopback.
pub struct DshClient {
    base: String,
    http: reqwest::blocking::Client,
}

impl DshClient {
    /// Build a client for `127.0.0.1:<port>`.
    ///
    /// The host is fixed: dsh's `/api` fence only admits loopback authorities
    /// (or an explicitly declared `--trusted-host`), and Fleet always runs the
    /// server it talks to on the same machine.
    pub fn new(port: u16) -> Result<Self, DshRpcError> {
        let http = reqwest::blocking::Client::builder()
            .timeout(DEFAULT_TIMEOUT)
            .build()
            .map_err(|e| DshRpcError::Transport(format!("http client: {e}")))?;
        Ok(Self {
            base: format!("http://127.0.0.1:{port}/api"),
            http,
        })
    }

    /// The `/api` base URL this client targets.
    pub fn base_url(&self) -> &str {
        &self.base
    }

    /// Invoke one unary method and return its `result.value`.
    pub fn call(&self, method: &str, payload: Value) -> Result<Value, DshRpcError> {
        let rpc_id = uuid::Uuid::new_v4().to_string();
        let body = build_request(&rpc_id, method, &payload);

        let resp = self
            .http
            .post(format!("{}/{}", self.base, method))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .map_err(|e| DshRpcError::Transport(format!("{method}: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| DshRpcError::Transport(format!("{method} body: {e}")))?;

        if !status.is_success() {
            return Err(DshRpcError::Transport(format!(
                "{method}: HTTP {status}{}",
                if status.as_u16() == 403 {
                    " (loopback trust fence)"
                } else {
                    ""
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
        let body = json!({
            "type": "client-response",
            "rpcId": rpc_id,
            "result": { "ok": true, "value": value },
        })
        .to_string();

        let resp = self
            .http
            .post(format!("{}/respond", self.base))
            .header("content-type", "application/json")
            .body(body)
            .send()
            .map_err(|e| DshRpcError::Transport(format!("respond: {e}")))?;

        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| DshRpcError::Transport(format!("respond body: {e}")))?;

        if !status.is_success() {
            return Err(DshRpcError::Transport(format!("respond: HTTP {status}")));
        }

        parse_receipt(&text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_request_carries_method_and_id() {
        let body = build_request("id-1", "session.list", &json!({}));
        let parsed: Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["type"], "client-request");
        assert_eq!(parsed["rpcId"], "id-1");
        assert_eq!(parsed["method"], "session.list");
        assert_eq!(parsed["payload"], json!({}));
    }

    #[test]
    fn parse_response_returns_ok_value() {
        // Shape taken verbatim from a live `session.create` response.
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
        // Shape taken verbatim from a live `credentials.describe` rejection.
        let body = r#"{"type":"server-response","rpcId":"id-1","result":{"ok":false,
            "error":{"code":"bad-request","message":"invalid payload for credentials.describe"}}}"#;
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
    fn client_targets_loopback() {
        let client = DshClient::new(3080).unwrap();
        assert_eq!(client.base_url(), "http://127.0.0.1:3080/api");
    }
}
