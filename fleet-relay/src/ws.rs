//! WebSocket endpoint: first frame authenticates (role + secret), then frames
//! are routed per role until the socket closes.
//!
//! Under the mobile end-to-end encryption (方案A), the `secret` here is the
//! HKDF-derived **channel token**, never the pairing secret — and every `msg`
//! payload is AES-256-GCM ciphertext (`{enc:"box",…}`). The relay is a blind
//! forwarder: it only ever sees the opaque channel token (which it hashes into
//! a routing bucket) and sealed bytes it passes through verbatim. It holds no
//! key and does no crypto; do not add any decrypt/encrypt logic here.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::unbounded_channel;

use crate::frames::{InFrame, OutFrame, PushPayload, Role};
use crate::registry::{channel_id, OutMsg};
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
/// in this comment rather than importing the constant.
const MAX_WS_MESSAGE_BYTES: usize = 32 * 1024 * 1024;

pub async fn ws_handler(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(MAX_WS_MESSAGE_BYTES)
        .max_frame_size(MAX_WS_MESSAGE_BYTES)
        .on_upgrade(move |socket| handle_socket(state, socket))
}

async fn handle_socket(state: Arc<AppState>, mut socket: WebSocket) {
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
    let joined = state.registry.join(&channel, role, tx);
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
            InFrame::Msg { payload } => {
                state.registry.forward(&channel, role, &OutFrame::Msg { payload });
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
