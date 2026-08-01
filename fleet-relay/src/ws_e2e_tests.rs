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
    spawn_server_with(crate::ws::DEFAULT_MAX_WS_MESSAGE_BYTES).await
}

async fn spawn_server_with(max_ws_message_bytes: usize) -> String {
    let dir = tempfile::tempdir().unwrap();
    let push = Push::new(
        Push::generate_private_key(),
        "mailto:t@example.com".into(),
        dir.path(),
    )
    .unwrap();
    // keep the tempdir alive for the process lifetime; tests are short-lived
    std::mem::forget(dir);
    // Caps set effectively unlimited: these tests exercise forwarding, not admission.
    let conn_limiter = crate::limits::ConnLimiter::new(100_000, 100_000);
    let conn_rate = crate::limits::ConnRateLimiter::new(100_000, 1e9);
    let state = Arc::new(AppState {
        registry: Registry::default(),
        push,
        harmony: None,
        conn_limiter,
        conn_rate,
        max_ws_message_bytes,
    });
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

/// Read frames until one of `want_type` arrives. Membership frames
/// (`presence` / `agent_status`) interleave with everything else, so a test that
/// takes "the next frame" is racing the registry's broadcasts.
async fn recv_json_of_type(ws: &mut Client, want_type: &str) -> Value {
    for _ in 0..8 {
        let v = recv_json(ws).await;
        if v["type"] == want_type {
            return v;
        }
    }
    panic!("no {want_type} frame arrived within 8 frames");
}

/// End-to-end custody: a client that answers while the desktop is away gets a
/// `queued` ack instead of silence, and the frame is handed to the agent that
/// joins afterwards. This is the whole point of the store-and-forward path — the
/// phone's socket usually dies long before a desktop reconnect, so without it
/// the answer evaporates while decision cards keep arriving.
///
/// Written after the implementation (the red/green pair for the queueing logic
/// itself lives in `registry::tests`); verified non-vacuous by temporarily
/// restoring the old drop-on-forward behaviour, which turns both halves red.
#[tokio::test]
async fn client_answer_is_acked_and_held_while_the_agent_is_away() {
    let url = spawn_server().await;

    // No agent on the channel at all — the old code silently dropped this.
    let (mut client, authed) = connect(&url, "client", SECRET).await;
    assert_eq!(authed["agent_online"], false);

    client
        .send(Message::Text(
            json!({"type": "msg", "payload": {"answer": "allow"}, "ack_id": "a1"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();

    let ack = recv_json(&mut client).await;
    assert_eq!(ack["type"], "msg_ack", "an acked client msg must get a custody report");
    assert_eq!(ack["ack_id"], "a1");
    assert_eq!(ack["status"], "queued", "no agent online → the relay holds it");

    // The desktop comes back and inherits the answer.
    let (mut agent, _) = connect(&url, "agent", SECRET).await;
    let got = recv_json(&mut agent).await;
    assert_eq!(got["type"], "msg");
    assert_eq!(got["payload"]["answer"], "allow", "the joining agent inherits the held answer");

    // With the agent online the same frame is now reported as delivered. The
    // client's stream also carries the `agent_status` bump from that join, so
    // wait for the ack specifically rather than taking the next frame blindly.
    client
        .send(Message::Text(
            json!({"type": "msg", "payload": {"answer": "deny"}, "ack_id": "a2"})
                .to_string()
                .into(),
        ))
        .await
        .unwrap();
    let ack = recv_json_of_type(&mut client, "msg_ack").await;
    assert_eq!(ack["ack_id"], "a2");
    assert_eq!(ack["status"], "delivered");
}

/// A client that sends no `ack_id` must behave exactly as before — no ack frame
/// is injected into its stream, or an older PWA would treat the relay's ack as
/// an agent reply and desync its pending-request map.
#[tokio::test]
async fn msg_without_ack_id_gets_no_ack_frame() {
    let url = spawn_server().await;
    let (mut agent, _) = connect(&url, "agent", SECRET).await;
    let (mut client, _) = connect(&url, "client", SECRET).await;
    let _presence = recv_json(&mut agent).await;

    client
        .send(Message::Text(json!({"type": "msg", "payload": {"x": 1}}).to_string().into()))
        .await
        .unwrap();

    // The agent still receives it…
    let got = recv_json(&mut agent).await;
    assert_eq!(got["payload"]["x"], 1);
    // …and nothing comes back down the client's own socket.
    let quiet = tokio::time::timeout(std::time::Duration::from_millis(300), client.next()).await;
    assert!(quiet.is_err(), "no ack was requested, so none may be sent");
}

#[tokio::test]
async fn auth_then_forward_between_roles() {
    let url = spawn_server().await;

    let (mut agent, authed) = connect(&url, "agent", SECRET).await;
    assert_eq!(authed["type"], "authed");
    assert_eq!(authed["clients"], 0);
    assert_eq!(authed["binary"], true, "relay must advertise binary forwarding");

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

/// A binary WebSocket frame is forwarded verbatim to the opposite role — the
/// transport path compressed `msg` payloads ride on top of.
#[tokio::test]
async fn binary_frame_forwarded_verbatim_between_roles() {
    let url = spawn_server().await;

    let (mut agent, _) = connect(&url, "agent", SECRET).await;
    let (mut client, _) = connect(&url, "client", SECRET).await;
    // drain the agent's presence bump from the client joining
    let _ = recv_json(&mut agent).await;

    let blob = vec![0x1f, 0x8b, 0x08, 0x00, 0x00, 0x11, 0x22, 0x33, 0xff];
    agent.send(Message::Binary(blob.clone().into())).await.unwrap();
    let got = recv_binary(&mut client).await;
    assert_eq!(got, blob, "agent binary frame reaches client byte-for-byte");

    // and the reverse direction
    client.send(Message::Binary(blob.clone().into())).await.unwrap();
    let got = recv_binary(&mut agent).await;
    assert_eq!(got, blob, "client binary frame reaches agent byte-for-byte");
}

async fn recv_binary(ws: &mut Client) -> Vec<u8> {
    loop {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(5), ws.next())
            .await
            .expect("timed out waiting for frame")
            .expect("socket closed")
            .expect("socket error");
        match msg {
            Message::Binary(b) => return b.to_vec(),
            Message::Text(_) => continue, // skip presence/status chatter
            _ => continue,
        }
    }
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

/// Full mobile end-to-end encryption (方案A) through the REAL relay: a payload
/// sealed with the desktop/phone crypto survives a round-trip in both
/// directions, and the frame the relay forwards is ciphertext only — the relay
/// never sees the pairing secret (it auths with the derived channel token) nor
/// the plaintext body. Guards the blind-forwarder contract this whole feature
/// rests on. The seal/open primitives and cross-language vectors are unit-tested
/// in claw-fleet-core::relay_crypto / mobile-web relayCrypto.test.ts; this proves
/// the two halves compose over a live socket.
#[tokio::test]
async fn sealed_payload_round_trips_through_relay() {
    use claw_fleet_core::relay_crypto::{derive_keys, open, seal, SealedBox};

    let url = spawn_server().await;
    // The pairing secret rides only in the QR fragment; the relay is handed the
    // derived channel token, never this.
    let keys = derive_keys("boss-pairing-secret-abcdef0123456789");
    assert_ne!(keys.channel_token, "boss-pairing-secret-abcdef0123456789");

    let (mut agent, _) = connect(&url, "agent", &keys.channel_token).await;
    let (mut client, _) = connect(&url, "client", &keys.channel_token).await;
    // drain the agent's presence bump from the client joining
    let _ = recv_json(&mut agent).await;

    // --- desktop -> phone --------------------------------------------------
    const DOWN_MARKER: &str = "TOP-SECRET-WORKSPACE-机密";
    let downlink = json!({"event": "sessions", "sessions": [{"id": "s1", "name": DOWN_MARKER}]});
    let sealed = serde_json::to_value(seal(&keys.enc_key, downlink.to_string().as_bytes())).unwrap();
    agent
        .send(Message::Text(json!({"type": "msg", "payload": sealed}).to_string().into()))
        .await
        .unwrap();
    let got = recv_json(&mut client).await;
    assert_eq!(got["type"], "msg");
    assert_eq!(got["payload"]["enc"], "box", "relay forwards a sealed envelope");
    assert!(
        !got.to_string().contains(DOWN_MARKER),
        "plaintext must not appear in the frame the relay forwarded: {got}"
    );
    let opened: SealedBox = serde_json::from_value(got["payload"].clone()).unwrap();
    let plain = open(&keys.enc_key, &opened).expect("phone opens the sealed downlink");
    assert_eq!(serde_json::from_slice::<Value>(&plain).unwrap(), downlink);

    // --- phone -> desktop --------------------------------------------------
    const UP_MARKER: &str = "APPROVED-老板批准";
    let uplink = json!({"event": "answer", "kind": "guard", "id": "req-42", "note": UP_MARKER});
    let sealed = serde_json::to_value(seal(&keys.enc_key, uplink.to_string().as_bytes())).unwrap();
    client
        .send(Message::Text(json!({"type": "msg", "payload": sealed}).to_string().into()))
        .await
        .unwrap();
    let got = recv_json(&mut agent).await;
    assert_eq!(got["payload"]["enc"], "box");
    assert!(
        !got.to_string().contains(UP_MARKER),
        "plaintext must not appear in the forwarded uplink frame: {got}"
    );
    let opened: SealedBox = serde_json::from_value(got["payload"].clone()).unwrap();
    let plain = open(&keys.enc_key, &opened).expect("desktop opens the sealed uplink");
    assert_eq!(serde_json::from_slice::<Value>(&plain).unwrap(), uplink);

    // A different pairing secret derives a different key that cannot open the
    // ciphertext the relay just forwarded — confirms confidentiality end-to-end.
    let wrong = derive_keys("some-other-secret-0000000000000000");
    assert!(open(&wrong.enc_key, &opened).is_err());
}

/// A message over the size cap is rejected by the transport and never forwarded
/// to the opposite role — confirms `WebSocketUpgrade::max_message_size` is
/// actually wired (the one hardening cap that's pure axum config, not our own
/// admission logic).
#[tokio::test]
async fn oversized_message_is_rejected_not_forwarded() {
    use std::time::Duration;
    // 1 KiB cap; the auth frame (~90 bytes) still fits so the socket authenticates.
    let url = spawn_server_with(1024).await;

    let (mut agent, _) = connect(&url, "agent", SECRET).await;
    let (mut client, _) = connect(&url, "client", SECRET).await;
    // drain the agent's presence bump from the client joining
    let _ = recv_json(&mut agent).await;

    // Agent sends a msg far over the 1 KiB cap.
    let big = "x".repeat(8 * 1024);
    let _ = agent
        .send(Message::Text(json!({"type": "msg", "payload": {"blob": big}}).to_string().into()))
        .await;

    // The relay must not forward it: the client sees no `msg` frame.
    let forwarded = tokio::time::timeout(Duration::from_millis(400), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Text(t))) => {
                    let v: Value = serde_json::from_str(&t).unwrap();
                    if v["type"] == "msg" {
                        return true;
                    }
                }
                Some(Ok(_)) => continue, // presence/status chatter
                _ => return false,       // socket closed → nothing forwarded
            }
        }
    })
    .await;
    assert!(
        !matches!(forwarded, Ok(true)),
        "oversized frame must not be forwarded past the size cap"
    );

    // And the offending sender's connection is torn down by the transport.
    let after = tokio::time::timeout(Duration::from_secs(2), agent.next()).await;
    assert!(
        matches!(after, Ok(None) | Ok(Some(Err(_))) | Ok(Some(Ok(Message::Close(_))))),
        "the oversized frame should close the sender's connection, got {after:?}"
    );
}
