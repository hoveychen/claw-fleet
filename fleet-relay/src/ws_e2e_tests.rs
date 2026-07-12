//! End-to-end WebSocket tests: real sockets through the axum handler.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use crate::push::Push;
use crate::registry::Registry;
use crate::{build_api, AppState};

type Client = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

const SECRET: &str = "0123456789abcdef0123456789abcdef";

async fn spawn_server() -> String {
    let dir = tempfile::tempdir().unwrap();
    let push = Push::new(
        Push::generate_private_key(),
        "mailto:t@example.com".into(),
        dir.path(),
    )
    .unwrap();
    // keep the tempdir alive for the process lifetime; tests are short-lived
    std::mem::forget(dir);
    let state = Arc::new(AppState { registry: Registry::default(), push });
    let app = build_api(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("ws://{addr}/ws")
}

async fn connect(url: &str, role: &str, secret: &str) -> (Client, Value) {
    let (mut ws, _) = connect_async(url).await.unwrap();
    ws.send(Message::Text(
        json!({"type": "auth", "role": role, "secret": secret}).to_string().into(),
    ))
    .await
    .unwrap();
    let reply = recv_json(&mut ws).await;
    (ws, reply)
}

async fn recv_json(ws: &mut Client) -> Value {
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for frame")
            .expect("socket closed")
            .expect("socket error");
        if let Message::Text(t) = msg {
            return serde_json::from_str(&t).unwrap();
        }
    }
}

#[tokio::test]
async fn auth_then_forward_between_roles() {
    let url = spawn_server().await;

    let (mut agent, authed) = connect(&url, "agent", SECRET).await;
    assert_eq!(authed["type"], "authed");
    assert_eq!(authed["clients"], 0);

    let (mut client, authed) = connect(&url, "client", SECRET).await;
    assert_eq!(authed["type"], "authed");
    assert_eq!(authed["agent_online"], true);

    // agent got the presence bump
    let presence = recv_json(&mut agent).await;
    assert_eq!(presence["type"], "presence");
    assert_eq!(presence["clients"], 1);

    // client -> agent
    client
        .send(Message::Text(json!({"type": "msg", "payload": {"hello": "agent"}}).to_string().into()))
        .await
        .unwrap();
    let got = recv_json(&mut agent).await;
    assert_eq!(got["type"], "msg");
    assert_eq!(got["payload"]["hello"], "agent");

    // agent -> client
    agent
        .send(Message::Text(json!({"type": "msg", "payload": {"hello": "client"}}).to_string().into()))
        .await
        .unwrap();
    let got = recv_json(&mut client).await;
    assert_eq!(got["payload"]["hello"], "client");

    // notify from agent is forwarded to clients
    agent
        .send(Message::Text(json!({"type": "notify", "title": "t", "body": "b"}).to_string().into()))
        .await
        .unwrap();
    let got = recv_json(&mut client).await;
    assert_eq!(got["type"], "notify");
    assert_eq!(got["title"], "t");
}

#[tokio::test]
async fn wrong_secret_is_isolated_and_short_secret_rejected() {
    let url = spawn_server().await;

    let (mut agent, _) = connect(&url, "agent", SECRET).await;
    let (mut other, _) = connect(&url, "client", "ffffffffffffffffffffffffffffffff").await;

    other
        .send(Message::Text(json!({"type": "msg", "payload": 1}).to_string().into()))
        .await
        .unwrap();
    // agent must not receive traffic from a different channel
    let res = tokio::time::timeout(std::time::Duration::from_millis(300), agent.next()).await;
    assert!(res.is_err(), "agent unexpectedly received cross-channel frame: {res:?}");

    let (mut ws, _) = connect_async(&url).await.unwrap();
    ws.send(Message::Text(json!({"type": "auth", "role": "client", "secret": "short"}).to_string().into()))
        .await
        .unwrap();
    let reply = recv_json(&mut ws).await;
    assert_eq!(reply["type"], "error");
}
