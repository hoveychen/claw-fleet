//! Route handlers extracted from the `hooks_server` request god-match
//! (Phase 4 module split). Zero-behavior-change extraction: each function
//! body is the verbatim body of its former `match path` arm, with shared
//! serve() state rebound from [`super::ServeCtx`] and arm-level `continue`
//! (which targeted the request loop) turned into `return`.
//! The uniform handler signature carries the full request context; not
//! every handler reads every field, hence the unused-variable allows.
#![allow(unused_variables, unused_mut, clippy::all)]
use super::*;

pub(crate) fn route_mobile_relay_config_get(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let cfg = crate::mobile_relay::load_config();
                let body = serde_json::to_string(&cfg).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

pub(crate) fn route_mobile_relay_config_post(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let mut body_bytes = Vec::new();
                let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
                let parsed: Result<crate::mobile_relay::MobileRelayConfig, _> =
                    serde_json::from_slice(&body_bytes);
                let (status, body) = match parsed {
                    Ok(cfg) => match crate::mobile_relay::set_config_normalized(cfg) {
                        Ok(stored) => (200, serde_json::to_string(&stored).unwrap_or_default()),
                        Err(e) => (500, serde_json::json!({"error": e}).to_string()),
                    },
                    Err(e) => (
                        400,
                        serde_json::json!({"error": format!("invalid body: {e}")}).to_string(),
                    ),
                };
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(status)
                        .with_header(json_header),
                );
            }

pub(crate) fn route_mobile_relay_rotate(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let (status, body) = match crate::mobile_relay::rotate_secret() {
                    Ok(cfg) => (200, serde_json::to_string(&cfg).unwrap_or_default()),
                    Err(e) => (500, serde_json::json!({"error": e}).to_string()),
                };
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(status)
                        .with_header(json_header),
                );
            }

pub(crate) fn route_mobile_relay_status(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let status = crate::mobile_relay::status();
                let body = serde_json::to_string(&status).unwrap_or_default();
                let _ = request.respond(
                    tiny_http::Response::from_string(body).with_header(json_header),
                );
            }

/// `POST /mobile_rpc` — the phone's data surface over plain HTTP.
///
/// A bridge, not a router: the body names one of
/// [`crate::mobile_relay::serve_request`]'s methods and this hands the answer
/// back in the same `{ok, data}` / `{ok, error}` envelope the relay's `reply`
/// frame uses, so a client can swap transports without reshaping anything it
/// reads. Only `req_id` is dropped — HTTP already pairs request with response.
///
/// A refused *method* stays a 200 with `ok:false`. The client distinguishes
/// "the host refused" from "the request never landed" (one is a message to
/// show, the other a retry), and folding the former into a 4xx would erase
/// that distinction. A malformed *body* is a real 400: nothing was dispatched.
pub(crate) fn route_mobile_rpc(
    ctx: &ServeCtx,
    mut request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
    // `cors`:跨源头(见 hooks_server::cors)。手机在设备簿里直连一台主机时,页面
    // 的 origin 是中转域名,少了这些头浏览器会在页面读到响应之前把它拦掉。
    cors: Vec<tiny_http::Header>,
) {
    let mut body_bytes = Vec::new();
    let _ = std::io::Read::read_to_end(&mut request.as_reader(), &mut body_bytes);
    let parsed: Result<serde_json::Value, _> = serde_json::from_slice(&body_bytes);
    let (status, body) = match parsed {
        Ok(payload) => {
            let method = payload
                .get("method")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("");
            let params = payload
                .get("params")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let reply = match crate::mobile_relay::serve_request(method, &params) {
                Ok(data) => serde_json::json!({"ok": true, "data": data}),
                Err(e) => serde_json::json!({"ok": false, "error": e}),
            };
            (200, reply.to_string())
        }
        Err(e) => (
            400,
            serde_json::json!({"ok": false, "error": format!("invalid body: {e}")}).to_string(),
        ),
    };
    let mut res = tiny_http::Response::from_string(body)
        .with_status_code(status)
        .with_header(json_header);
    for h in cors {
        res.add_header(h);
    }
    let _ = request.respond(res);
}

/// Text form of the pairing URL — what the desktop's 「复制配对链接」 button
/// copies. Separate from the QR route because a self-hosted relay can only be
/// paired by pasting (App Links need the host baked into the manifest).
pub(crate) fn route_mobile_relay_pairing_url(
    _ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    _path: &str,
) {
    let lang = query.get("lang").map(String::as_str);
    let (status, body) = match crate::mobile_relay::pairing_url_text(lang) {
        Ok(url) => (200, serde_json::json!({ "url": url }).to_string()),
        Err(e) => (404, serde_json::json!({ "error": e }).to_string()),
    };
    let _ = request.respond(
        tiny_http::Response::from_string(body)
            .with_status_code(status)
            .with_header(json_header),
    );
}

pub(crate) fn route_mobile_relay_qr(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let lang = query.get("lang").map(String::as_str);
                let (status, body) = match crate::mobile_relay::qr_svg(lang) {
                    Ok(svg) => (200, serde_json::json!({"svg": svg}).to_string()),
                    Err(e) => (404, serde_json::json!({"error": e}).to_string()),
                };
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(status)
                        .with_header(json_header),
                );
            }
