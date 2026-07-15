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
    Msg { payload: Value },
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
    /// Client only: remove a previously registered subscription for this
    /// channel. The payload identifies the sub the same way `PushSubscribe`
    /// does — a harmony sub by `platform:"harmony"` + `openId`, a web sub by
    /// `endpoint`. Removing an absent subscription is a no-op.
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
    Authed {
        role: Role,
        clients: usize,
        agent_online: bool,
        binary: bool,
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
    /// Sent to agents when the number of connected clients changes.
    Presence { clients: usize },
    /// Sent to clients when agent connectivity changes.
    AgentStatus { online: bool },
    Error { message: String },
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
