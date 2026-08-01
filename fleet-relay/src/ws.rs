//! WebSocket endpoint: first frame authenticates (role + secret), then frames
//! are routed per role until the socket closes.
//!
//! Under the mobile end-to-end encryption (方案A), the `secret` here is the
//! HKDF-derived **channel token**, never the pairing secret — and every `msg`
//! payload is AES-256-GCM ciphertext (`{enc:"box",…}`). The relay is a blind
//! forwarder: it only ever sees the opaque channel token (which it hashes into
//! a routing bucket) and sealed bytes it passes through verbatim. It holds no
//! key and does no crypto; do not add any decrypt/encrypt logic here.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::unbounded_channel;

use crate::frames::{InFrame, MsgAckStatus, OutFrame, PushPayload, Role};
use crate::limits::ConnGuard;
use crate::registry::{channel_id, Delivery, OutMsg};
use crate::AppState;

const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const MIN_SECRET_LEN: usize = 16;

/// Cap on a single WebSocket message (and frame). tungstenite's defaults are
/// 64 MiB per message / 16 MiB per frame; on a public multi-tenant relay a
/// 64 MiB-per-connection ceiling is a needless memory-exhaustion surface, so we
/// tighten it — but it must stay **above the largest legitimate frame** or it
/// would silently truncate real uploads.
///
/// Largest legitimate payload is a decision asset: `MAX_ASSET_BYTES` = 12 MiB
/// of raw bytes (claw-fleet-core `mobile_relay.rs`). The phone base64-encodes it
/// into the request JSON (×4/3 → 16 MiB), then E2E-seals it and base64-encodes
/// the ciphertext (×4/3 again → ~21 MiB), and sends it as **one** text `msg`
/// frame (`relay.ts` never fragments). So the on-wire max is ~21 MiB.
///
/// 32 MiB gives ~50% headroom over that while still halving tungstenite's
/// message default; we also raise the *frame* cap to the same 32 MiB (up from
/// the 16 MiB default) so a large single-frame upload can't trip the per-frame
/// limit. The relay is generic (no Fleet business deps), so the derivation lives
/// in this comment rather than importing the constant. Override via
/// `RELAY_MAX_WS_MESSAGE_BYTES`.
pub const DEFAULT_MAX_WS_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

pub async fn ws_handler(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> Response {
    let ip = client_ip(&headers);
    // Rate-limit new connections per IP first (cheapest rejection for a flood),
    // then reserve a concurrent slot. Unkeyed (no forwarded IP) skips the rate
    // gate — there's no bucket to key — and is bounded only by the global cap.
    if let Some(ip) = ip {
        if !state.conn_rate.check(ip) {
            log::warn!("connection rejected: rate limit (ip={ip})");
            return (StatusCode::TOO_MANY_REQUESTS, "too many connection attempts").into_response();
        }
    }
    let Some(guard) = state.conn_limiter.try_acquire(ip) else {
        log::warn!("connection rejected: at capacity (ip={ip:?})");
        return (StatusCode::SERVICE_UNAVAILABLE, "relay at capacity").into_response();
    };
    let max_bytes = state.max_ws_message_bytes;
    ws.max_message_size(max_bytes)
        .max_frame_size(max_bytes)
        .on_upgrade(move |socket| handle_socket(state, socket, guard))
}

/// Resolve the client IP for per-IP capping. Behind Traefik (which terminates
/// TLS) the TCP peer is always the proxy, so the real address must come from a
/// forwarded header — and only from one Traefik itself controls.
///
/// We use the **right-most** `X-Forwarded-For` entry. muvee's Traefik has no
/// `forwardedHeaders.trustedIPs`, so it owns the header: it discards/overwrites
/// any client-supplied XFF and appends the real connecting IP, which lands
/// right-most (verified against ~/workspace/muvee `traefik/traefik.yml` +
/// `handleTraefikConfig`, which adds no header-stripping middleware). The
/// left-most entry, by contrast, is whatever the client claimed — spoofable.
///
/// We deliberately do NOT trust `X-Real-Ip`: Traefik doesn't set it and doesn't
/// strip a client-supplied one, so a hostile client could rotate fake
/// `X-Real-Ip` values to dodge the per-IP cap. Absent XFF (a direct/dev
/// connection with no proxy), returns `None` and per-IP capping is skipped.
///
/// Caveat: this assumes exactly one trusted proxy hop. If a CDN in proxied mode
/// (e.g. Cloudflare) is ever placed in front of Traefik, all clients collapse to
/// the CDN's egress IPs and per-IP keying degrades toward the global cap. The
/// current deployment uses Traefik-direct ACME (httpChallenge), incompatible
/// with a proxying CDN, so that isn't the case today.
fn client_ip(headers: &HeaderMap) -> Option<IpAddr> {
    headers
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.rsplit(',').next())
        .and_then(|s| s.trim().parse::<IpAddr>().ok())
}

async fn handle_socket(state: Arc<AppState>, mut socket: WebSocket, _conn: ConnGuard) {
    // `_conn` holds this socket's reserved connection slot; dropping it when the
    // function returns (any exit path) releases the global + per-IP count.
    // --- auth: first text frame within the timeout ---
    let auth = tokio::time::timeout(AUTH_TIMEOUT, next_text(&mut socket)).await;
    let (role, secret) = match auth {
        Ok(Some(text)) => match serde_json::from_str::<InFrame>(&text) {
            // `secret` is the opaque channel token (64 hex chars, so the
            // ≥16 length gate always passes); the relay never learns the
            // pairing secret it was derived from.
            Ok(InFrame::Auth { role, secret }) if secret.len() >= MIN_SECRET_LEN => (role, secret),
            Ok(InFrame::Auth { .. }) => {
                let _ = send_frame(&mut socket, &OutFrame::Error { message: "secret too short".into() }).await;
                return;
            }
            _ => {
                let _ = send_frame(&mut socket, &OutFrame::Error { message: "expected auth frame".into() }).await;
                return;
            }
        },
        _ => return,
    };
    let channel = channel_id(&secret);

    let (tx, mut rx) = unbounded_channel::<OutMsg>();
    // Kept so this connection can push frames addressed to *itself* (the
    // `msg_ack` custody report) through the same write pump.
    let own_tx = tx.clone();
    let Some(joined) = state.registry.join(&channel, role, tx) else {
        let _ = send_frame(&mut socket, &OutFrame::Error { message: "channel at capacity".into() }).await;
        return;
    };
    log::info!("{role:?} joined channel {}… ({} client(s))", &channel[..12], joined.clients);
    let authed = OutFrame::Authed {
        role,
        clients: joined.clients,
        agent_online: joined.agent_online,
        binary: true,
    };
    if send_frame(&mut socket, &authed).await.is_err() {
        state.registry.leave(&channel, role, joined.conn_id);
        return;
    }

    let (mut sink, mut stream) = socket.split();

    // write pump: registry -> socket
    let write = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            let msg = match out {
                OutMsg::Text(s) => Message::Text(s.into()),
                OutMsg::Binary(b) => Message::Binary(b.into()),
            };
            if sink.send(msg).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // read loop: socket -> route
    while let Some(Ok(msg)) = stream.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
            // A binary frame is always an opaque `msg` payload (a compressed
            // envelope): forward the bytes verbatim to the opposite role. The
            // relay never decompresses or inspects them — only the paired
            // endpoints share the codec.
            Message::Binary(b) => {
                state.registry.forward_binary(&channel, role, b.to_vec());
                continue;
            }
            Message::Close(_) => break,
            _ => continue, // ping/pong handled by the stack
        };
        let frame = match serde_json::from_str::<InFrame>(&text) {
            Ok(f) => f,
            Err(e) => {
                log::debug!("bad frame on channel {}…: {e}", &channel[..12]);
                continue;
            }
        };
        match frame {
            InFrame::Auth { .. } => {} // already authed; ignore
            InFrame::Msg { payload, ack_id } => {
                let out = OutFrame::Msg { payload };
                match role {
                    // A client frame is taken into custody when the agent is
                    // away: the phone's socket usually lives only seconds, far
                    // less than a desktop reconnect, so dropping it here is what
                    // made answers vanish while cards still arrived.
                    Role::Client => {
                        let serialized = match serde_json::to_string(&out) {
                            Ok(s) => s,
                            Err(_) => continue,
                        };
                        let delivery =
                            state.registry.deliver_or_queue(&channel, OutMsg::Text(serialized));
                        if let Some(ack_id) = ack_id {
                            let status = match delivery {
                                Delivery::Delivered(_) => MsgAckStatus::Delivered,
                                Delivery::Queued(_) => MsgAckStatus::Queued,
                                Delivery::Dropped => MsgAckStatus::Dropped,
                            };
                            let ack = OutFrame::MsgAck { ack_id, status };
                            // Straight back down this socket: the ack belongs to
                            // this connection, not to the channel's other role.
                            if let Ok(s) = serde_json::to_string(&ack) {
                                let _ = own_tx.send(OutMsg::Text(s));
                            }
                        }
                    }
                    Role::Agent => {
                        state.registry.forward(&channel, role, &out);
                    }
                }
            }
            InFrame::Notify { title, body, tag, url } if role == Role::Agent => {
                let out = OutFrame::Notify {
                    title: title.clone(),
                    body: body.clone(),
                    tag: tag.clone(),
                    url: url.clone(),
                };
                state.registry.forward(&channel, role, &out);
                let payload = PushPayload {
                    title: &title,
                    body: &body,
                    tag: tag.as_deref(),
                    url: url.as_deref(),
                };
                state.push.notify(&channel, &payload, state.harmony.as_ref()).await;
            }
            InFrame::PushSubscribe { subscription } if role == Role::Client => {
                if let Err(e) = state.push.subscribe(&channel, subscription) {
                    log::warn!("push subscribe rejected on {}…: {e}", &channel[..12]);
                }
            }
            InFrame::PushUnsubscribe { subscription } if role == Role::Client => {
                if let Err(e) = state.push.unsubscribe(&channel, &subscription) {
                    log::warn!("push unsubscribe rejected on {}…: {e}", &channel[..12]);
                }
            }
            InFrame::Notify { .. }
            | InFrame::PushSubscribe { .. }
            | InFrame::PushUnsubscribe { .. } => {
                log::debug!("{role:?} sent a frame reserved for the opposite role; dropped");
            }
        }
    }

    state.registry.leave(&channel, role, joined.conn_id);
    write.abort();
    log::info!("{role:?} left channel {}…", &channel[..12]);
}

async fn next_text(socket: &mut WebSocket) -> Option<String> {
    while let Some(Ok(msg)) = socket.recv().await {
        match msg {
            Message::Text(t) => return Some(t.to_string()),
            Message::Close(_) => return None,
            _ => continue,
        }
    }
    None
}

async fn send_frame(socket: &mut WebSocket, frame: &OutFrame) -> Result<(), axum::Error> {
    let s = serde_json::to_string(frame).unwrap_or_default();
    socket.send(Message::Text(s.into())).await
}

#[cfg(test)]
mod tests {
    use super::client_ip;
    use axum::http::header::HeaderName;
    use axum::http::{HeaderMap, HeaderValue};

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                HeaderName::from_bytes(k.as_bytes()).unwrap(),
                HeaderValue::from_str(v).unwrap(),
            );
        }
        h
    }

    #[test]
    fn uses_rightmost_forwarded_for() {
        let ip = client_ip(&headers(&[("x-forwarded-for", "1.1.1.1, 2.2.2.2, 3.3.3.3")]));
        assert_eq!(ip.unwrap().to_string(), "3.3.3.3", "right-most (proxy-appended) entry wins");
    }

    #[test]
    fn ignores_spoofable_x_real_ip() {
        // A hostile client sets X-Real-Ip and a fake left-most XFF; Traefik
        // appends the true IP right-most. We must key on the right-most XFF and
        // ignore X-Real-Ip, or the per-IP cap is trivially bypassed.
        let ip = client_ip(&headers(&[
            ("x-real-ip", "9.9.9.9"),
            ("x-forwarded-for", "6.6.6.6, 4.4.4.4"),
        ]));
        assert_eq!(ip.unwrap().to_string(), "4.4.4.4", "X-Real-Ip must not override XFF");
    }

    #[test]
    fn x_real_ip_alone_is_not_trusted() {
        // No XFF (so no trusted proxy hop) → don't fall back to the spoofable
        // X-Real-Ip; skip per-IP capping instead.
        assert!(client_ip(&headers(&[("x-real-ip", "9.9.9.9")])).is_none());
    }

    #[test]
    fn no_forwarding_header_yields_none() {
        assert!(client_ip(&headers(&[])).is_none());
    }
}
