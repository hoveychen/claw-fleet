//! WebSocket endpoint: first frame authenticates (role + secret), then frames
//! are routed per role until the socket closes.

use std::sync::Arc;
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc::unbounded_channel;

use crate::frames::{InFrame, OutFrame, PushPayload, Role};
use crate::registry::channel_id;
use crate::AppState;

const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
const MIN_SECRET_LEN: usize = 16;

pub async fn ws_handler(State(state): State<Arc<AppState>>, ws: WebSocketUpgrade) -> Response {
    ws.on_upgrade(move |socket| handle_socket(state, socket))
}

async fn handle_socket(state: Arc<AppState>, mut socket: WebSocket) {
    // --- auth: first text frame within the timeout ---
    let auth = tokio::time::timeout(AUTH_TIMEOUT, next_text(&mut socket)).await;
    let (role, secret) = match auth {
        Ok(Some(text)) => match serde_json::from_str::<InFrame>(&text) {
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

    let (tx, mut rx) = unbounded_channel::<String>();
    let joined = state.registry.join(&channel, role, tx);
    log::info!("{role:?} joined channel {}… ({} client(s))", &channel[..12], joined.clients);
    let authed = OutFrame::Authed {
        role,
        clients: joined.clients,
        agent_online: joined.agent_online,
    };
    if send_frame(&mut socket, &authed).await.is_err() {
        state.registry.leave(&channel, role, joined.conn_id);
        return;
    }

    let (mut sink, mut stream) = socket.split();

    // write pump: registry -> socket
    let write = tokio::spawn(async move {
        while let Some(s) = rx.recv().await {
            if sink.send(Message::Text(s.into())).await.is_err() {
                break;
            }
        }
        let _ = sink.close().await;
    });

    // read loop: socket -> route
    while let Some(Ok(msg)) = stream.next().await {
        let text = match msg {
            Message::Text(t) => t.to_string(),
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
                state.push.notify(&channel, &payload).await;
            }
            InFrame::PushSubscribe { subscription } if role == Role::Client => {
                if let Err(e) = state.push.subscribe(&channel, subscription) {
                    log::warn!("push subscribe rejected on {}…: {e}", &channel[..12]);
                }
            }
            InFrame::Notify { .. } | InFrame::PushSubscribe { .. } => {
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
