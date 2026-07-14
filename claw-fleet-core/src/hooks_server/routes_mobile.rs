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

pub(crate) fn route_mobile_relay_qr(
    ctx: &ServeCtx,
    request: tiny_http::Request,
    query: &std::collections::HashMap<String, String>,
    json_header: tiny_http::Header,
    path: &str,
) {

                let (status, body) = match crate::mobile_relay::qr_svg() {
                    Ok(svg) => (200, serde_json::json!({"svg": svg}).to_string()),
                    Err(e) => (404, serde_json::json!({"error": e}).to_string()),
                };
                let _ = request.respond(
                    tiny_http::Response::from_string(body)
                        .with_status_code(status)
                        .with_header(json_header),
                );
            }
