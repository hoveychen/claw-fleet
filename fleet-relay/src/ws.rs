//! WebSocket endpoint: first frame authenticates (role + secret), then frames
//! are routed per role until the socket closes.
//!
//! Under mobile end-to-end encryption, the `secret` here is the HKDF-derived
//! **channel token**, never the pairing secret — and every `msg` payload is
//! AES-256-GCM ciphertext (`{enc:"box",…}`). The relay is a blind forwarder: it
//! only ever sees the opaque channel token (which it hashes into a routing bucket)
//! and sealed bytes it passes through verbatim. It holds no key and does no crypto;
//! do not add any decrypt/encrypt logic here.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use futures_util::{SinkExt, StreamExt};
// Fully qualified at the call site: `channel` is also the local variable
// holding this connection's channel id.
use tokio::sync::mpsc;

use crate::frames::{InFrame, MsgAckStatus, OutFrame, PushPayload, Role};
use crate::limits::ConnGuard;
use crate::notify_target;
use crate::registry::{channel_id, Delivery, OutMsg, OUT_QUEUE_CAP};
use crate::AppState;

const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const MIN_SECRET_LEN: usize = 16;

/// How often the relay pings each authed connection.
///
/// Without this the relay had no liveness signal at all: a peer whose TCP
/// connection is half-open (phone changed networks, agent machine slept) keeps
/// its registry entry, keeps counting as online, and keeps being handed frames
/// that vanish — until the OS TCP keepalive notices, which on Linux defaults to
/// roughly two hours. Every frame routed to it in the meantime is lost with the
/// sender told `delivered`.
const PING_INTERVAL: Duration = Duration::from_secs(30);

/// Drop a connection that has produced nothing — not a frame, not a pong — for
/// this long. Three missed pings; a link that cannot answer any of them is not
/// coming back on its own. Matches the desktop agent's own 90s budget
/// (`claw-fleet-core/src/mobile_relay.rs`) so both ends of a dead link give up
/// on roughly the same schedule.
const IDLE_TIMEOUT: Duration = Duration::from_secs(90);

/// How long one frame may sit inside `sink.send` before the connection is
/// written off.
///
/// A peer that stops reading does not error — TCP's receive window closes and
/// the send simply never resolves. The write pump then parks forever while the
/// registry keeps handing it frames and telling the sender `delivered`. Shorter
/// than [`IDLE_TIMEOUT`] so a stalled writer is noticed on its own terms rather
/// than waiting out the read side's clock.
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

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
            return (
                StatusCode::TOO_MANY_REQUESTS,
                "too many connection attempts",
            )
                .into_response();
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
                let _ = send_frame(
                    &mut socket,
                    &OutFrame::Error {
                        message: "secret too short".into(),
                    },
                )
                .await;
                return;
            }
            _ => {
                let _ = send_frame(
                    &mut socket,
                    &OutFrame::Error {
                        message: "expected auth frame".into(),
                    },
                )
                .await;
                return;
            }
        },
        _ => return,
    };
    let channel = channel_id(&secret);

    let (tx, mut rx) = mpsc::channel::<OutMsg>(OUT_QUEUE_CAP);
    // Kept so this connection can push frames addressed to *itself* (the
    // `msg_ack` custody report) through the same write pump.
    let own_tx = tx.clone();
    let Some(joined) = state.registry.join(&channel, role, tx) else {
        let _ = send_frame(
            &mut socket,
            &OutFrame::Error {
                message: "channel at capacity".into(),
            },
        )
        .await;
        return;
    };
    log::info!(
        "{role:?} joined channel {}… ({} client(s))",
        &channel[..12],
        joined.clients
    );
    let authed = OutFrame::Authed {
        role,
        clients: joined.clients,
        agent_online: joined.agent_online,
        binary: true,
        pong: true,
    };
    if send_frame(&mut socket, &authed).await.is_err() {
        state.registry.leave(&channel, role, joined.conn_id);
        return;
    }

    let (mut sink, mut stream) = socket.split();

    // write pump: registry -> socket
    let write_channel = channel.clone();
    let write = tokio::spawn(async move {
        while let Some(out) = rx.recv().await {
            let msg = match out {
                OutMsg::Text(s) => Message::Text(s.into()),
                OutMsg::Binary(b) => Message::Binary(b.into()),
                OutMsg::Ping => Message::Ping(Vec::new().into()),
            };
            // Bounded: a peer that stopped reading parks this send forever (see
            // `WRITE_TIMEOUT`). Giving up drops `rx`, which is what makes the
            // registry's `try_send` start failing for this connection instead of
            // reporting frames as delivered into a queue nothing drains.
            match tokio::time::timeout(WRITE_TIMEOUT, sink.send(msg)).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => break,
                Err(_) => {
                    log::info!(
                        "write stalled {}s on channel {}…; dropping connection",
                        WRITE_TIMEOUT.as_secs(),
                        &write_channel[..12]
                    );
                    break;
                }
            }
        }
        let _ = sink.close().await;
    });

    // read loop: socket -> route, with its own liveness clock.
    //
    // The loop cannot simply await `stream.next()`: a half-open socket never
    // yields anything and never errors, so the connection would sit in the
    // registry indefinitely. Pinging on a timer and dropping the connection
    // once nothing has come back for `IDLE_TIMEOUT` is what bounds that.
    // `interval_at`, not `interval`: the latter's first tick fires immediately,
    // which would put a ping on the wire before the connection has said
    // anything. A socket that just completed auth is self-evidently alive.
    let mut ping =
        tokio::time::interval_at(tokio::time::Instant::now() + PING_INTERVAL, PING_INTERVAL);
    ping.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_inbound = std::time::Instant::now();
    loop {
        let msg = tokio::select! {
            next = stream.next() => match next {
                Some(Ok(m)) => m,
                // Stream ended or errored — the normal close path.
                _ => break,
            },
            _ = ping.tick() => {
                // A closed write pump means the socket is already gone; a full
                // one means it is parked mid-send and never coming back.
                if own_tx.try_send(OutMsg::Ping).is_err() {
                    break;
                }
                if last_inbound.elapsed() > IDLE_TIMEOUT {
                    log::info!(
                        "{role:?} idle {}s on channel {}…; dropping half-open connection",
                        last_inbound.elapsed().as_secs(),
                        &channel[..12]
                    );
                    break;
                }
                continue;
            }
        };
        // Any inbound traffic proves the link works — a pong counts, which is
        // the whole point of sending the pings above.
        last_inbound = std::time::Instant::now();
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
            // Answered on this socket, never forwarded: the probe asks whether
            // *this* connection round-trips.
            InFrame::Ping { id } => {
                if let Ok(s) = serde_json::to_string(&OutFrame::Pong { id }) {
                    let _ = own_tx.try_send(OutMsg::Text(s));
                }
            }
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
                        let delivery = state
                            .registry
                            .deliver_or_queue(&channel, OutMsg::Text(serialized));
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
                                let _ = own_tx.try_send(OutMsg::Text(s));
                            }
                        }
                    }
                    Role::Agent => {
                        state.registry.forward(&channel, role, &out);
                    }
                }
            }
            InFrame::Notify {
                title,
                body,
                tag,
                url,
                badge,
            } if role == Role::Agent => {
                // Stamp which channel this notification came from. One phone can pair with
                // multiple desktops, and each desktop's URL carries only the card id — but the
                // card id is unique only within one machine. When two desktops both have a card,
                // it's unclear which one opens when clicked. The relay is the only place that
                // knows which channel a message comes from at fan-out time, and this way the
                // desktop doesn't need to change anything.
                //
                // We stamp the version sent to online clients too: on that path, the phone can
                // actually infer the device from which socket the frame travels, but only if
                // both paths produce the same URL will click behavior not depend on whether
                // the client is currently online.
                let stamped = notify_target::stamp_channel(url.as_deref(), &channel);
                let out = OutFrame::Notify {
                    title: title.clone(),
                    body: body.clone(),
                    tag: tag.clone(),
                    url: stamped.clone(),
                    badge,
                };
                state.registry.forward(&channel, role, &out);
                let payload = PushPayload {
                    title: &title,
                    body: &body,
                    tag: tag.as_deref(),
                    url: stamped.as_deref(),
                    badge,
                };
                state
                    .push
                    .notify(&channel, &payload, state.harmony.as_ref())
                    .await;
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
        let ip = client_ip(&headers(&[(
            "x-forwarded-for",
            "1.1.1.1, 2.2.2.2, 3.3.3.3",
        )]));
        assert_eq!(
            ip.unwrap().to_string(),
            "3.3.3.3",
            "right-most (proxy-appended) entry wins"
        );
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
        assert_eq!(
            ip.unwrap().to_string(),
            "4.4.4.4",
            "X-Real-Ip must not override XFF"
        );
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
