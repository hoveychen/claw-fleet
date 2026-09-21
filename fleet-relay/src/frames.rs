//! Wire protocol frames. The relay understands only these envelope types;
//! `msg` payloads are opaque JSON owned by the paired endpoints.

use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Agent,
    Client,
}

impl Role {
    pub fn opposite(self) -> Role {
        match self {
            Role::Agent => Role::Client,
            Role::Client => Role::Agent,
        }
    }
}

/// Frames the relay accepts from a connection.
#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InFrame {
    /// Must be the first frame on every connection.
    Auth { role: Role, secret: String },
    /// Opaque payload forwarded verbatim to the opposite role.
    ///
    /// `ack_id` is an optional, deliberately meaningless correlation id the
    /// sender may attach so the relay can report back what became of the frame
    /// (`msg_ack`). It sits *outside* the sealed payload because the relay
    /// cannot open the envelope — the `req_id` the endpoints correlate on is
    /// ciphertext to us. Older clients omit it and get no ack, as before.
    Msg {
        payload: Value,
        #[serde(default)]
        ack_id: Option<String>,
    },
    /// Agent only: fan out a Web Push to the channel's subscriptions and
    /// forward to online clients.
    Notify {
        title: String,
        body: String,
        #[serde(default)]
        tag: Option<String>,
        #[serde(default)]
        url: Option<String>,
    },
    /// Client only: register a browser PushSubscription for this channel.
    PushSubscribe { subscription: Value },
    /// Liveness probe from either role, answered with `pong` on the same
    /// socket. A browser cannot send protocol-level pings (the WebSocket API
    /// exposes no such call), so a phone has no way to tell a working link from
    /// a half-open one that still reports `readyState === OPEN`. This gives it
    /// one: no `pong` within its budget means the socket is dead regardless of
    /// what the browser claims. `id` is echoed back so the sender can match a
    /// reply to the probe it sent rather than to a stale one.
    Ping {
        #[serde(default)]
        id: Option<String>,
    },
    /// Client only: remove a previously registered subscription for this
    /// channel. The payload identifies the sub the same way `PushSubscribe`
    /// does — a harmony sub by `platform:"harmony"` plus either a device
    /// `token` (general app channel) or an `openId` (service app channel),
    /// a web sub by `endpoint`. Removing an absent subscription is a no-op.
    PushUnsubscribe { subscription: Value },
}

/// Frames the relay emits to a connection.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum OutFrame {
    /// Auth acknowledgement with the initial channel state. `binary` advertises
    /// that this relay forwards WebSocket binary frames verbatim between paired
    /// endpoints — an agent uses it to decide whether it may ship compressed
    /// `msg` payloads as raw bytes instead of base64-in-JSON. Older relays omit
    /// the field, so `#[serde(default)]` on the reader yields `false` and the
    /// agent falls back to text transport.
    /// `pong` advertises that this relay answers `InFrame::Ping`. A phone must
    /// not probe a relay that predates the frame: an older build fails to parse
    /// it, logs, and drops it silently, so every probe would look like a dead
    /// link and the phone would reconnect in a loop over a perfectly good
    /// socket. Absent on old relays → `#[serde(default)]` on the reader yields
    /// `false` and the phone leaves its probe off, behaving exactly as before.
    Authed {
        role: Role,
        clients: usize,
        agent_online: bool,
        binary: bool,
        pong: bool,
    },
    Msg { payload: Value },
    Notify {
        title: String,
        body: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        tag: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        url: Option<String>,
    },
    /// Custody report for a client `msg` that carried an `ack_id`.
    ///
    /// `Delivered` means live agent connections received it. `Queued` means no
    /// agent was online and the relay is holding it for the next one — so the
    /// sender may stop retrying, but must not read it as "the desktop has acted
    /// on this". `Dropped` means neither happened and the sender should retry.
    MsgAck {
        ack_id: String,
        status: MsgAckStatus,
    },
    /// Sent to agents when the number of connected clients changes.
    Presence { clients: usize },
    /// Sent to clients when agent connectivity changes.
    AgentStatus { online: bool },
    /// Answer to an `InFrame::Ping`, echoing its `id`. Goes straight back down
    /// the socket it arrived on — it proves *this* connection round-trips, so
    /// forwarding it to the opposite role would prove nothing.
    Pong {
        #[serde(skip_serializing_if = "Option::is_none")]
        id: Option<String>,
    },
    Error { message: String },
}

/// What the relay did with an acked client `msg`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MsgAckStatus {
    Delivered,
    Queued,
    Dropped,
}

/// Payload of a Web Push notification (what the service worker receives).
#[derive(Debug, Serialize)]
pub struct PushPayload<'a> {
    pub title: &'a str,
    pub body: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tag: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub url: Option<&'a str>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // The phone builds these frames by hand in TypeScript
    // (`mobile-web/src/relay.ts`), so the wire shape is a contract, not an
    // implementation detail: a rename here silently strands the phone's
    // liveness probe and it would go back to trusting a half-open socket.

    #[test]
    fn ping_frame_parses_with_and_without_id() {
        let with = serde_json::from_str::<InFrame>(r#"{"type":"ping","id":"p7"}"#).unwrap();
        assert!(matches!(with, InFrame::Ping { id: Some(ref s) } if s == "p7"));
        let without = serde_json::from_str::<InFrame>(r#"{"type":"ping"}"#).unwrap();
        assert!(matches!(without, InFrame::Ping { id: None }));
    }

    #[test]
    fn pong_serializes_with_echoed_id() {
        let s = serde_json::to_string(&OutFrame::Pong { id: Some("p7".into()) }).unwrap();
        assert_eq!(s, r#"{"type":"pong","id":"p7"}"#);
    }

    #[test]
    fn pong_omits_absent_id() {
        let s = serde_json::to_string(&OutFrame::Pong { id: None }).unwrap();
        assert_eq!(s, r#"{"type":"pong"}"#, "an absent id must not serialize as null");
    }
}
