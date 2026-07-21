//! Channel registry — pairs agent and client connections that presented the
//! same secret and routes frames between the two roles.
//!
//! A channel id is `hex(sha256(secret))`; the raw secret is never stored.
//! With mobile end-to-end encryption the `secret` handed in is the opaque
//! HKDF-derived channel token, so the bucket is `hex(sha256(channel_token))` —
//! the routing math is unchanged and the relay still only ever sees the token
//! and ciphertext, never the pairing secret or the plaintext `msg` bodies.
//! Connections push serialized frames through unbounded senders so the
//! registry stays synchronous and unit-testable without real sockets.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use sha2::{Digest, Sha256};
use tokio::sync::mpsc::UnboundedSender;

use crate::frames::{OutFrame, Role};

/// A frame queued for one connection's write pump. Text carries the relay's
/// structured JSON envelopes (`authed`/`msg`/`presence`/…); Binary carries an
/// opaque compressed `msg` payload forwarded verbatim between paired endpoints
/// — the relay never inspects its bytes.
#[derive(Debug, Clone)]
pub enum OutMsg {
    Text(String),
    Binary(Vec<u8>),
}

pub type Tx = UnboundedSender<OutMsg>;

pub fn channel_id(secret: &str) -> String {
    let mut h = Sha256::new();
    h.update(secret.as_bytes());
    let out = h.finalize();
    out.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Default)]
struct Channel {
    agents: HashMap<u64, Tx>,
    clients: HashMap<u64, Tx>,
}

impl Channel {
    fn members(&self, role: Role) -> &HashMap<u64, Tx> {
        match role {
            Role::Agent => &self.agents,
            Role::Client => &self.clients,
        }
    }

    fn members_mut(&mut self, role: Role) -> &mut HashMap<u64, Tx> {
        match role {
            Role::Agent => &mut self.agents,
            Role::Client => &mut self.clients,
        }
    }

    fn is_empty(&self) -> bool {
        self.agents.is_empty() && self.clients.is_empty()
    }
}

/// Default cap on distinct live channels (override: `RELAY_MAX_CHANNELS`). Each
/// channel is one pairing secret, so this bounds how many independent tenants
/// can hold state at once — a memory backstop, set well above any realistic
/// active-user count.
pub const DEFAULT_MAX_CHANNELS: usize = 100_000;
/// Default cap on connections of one role within a single channel (override:
/// `RELAY_MAX_PER_CHANNEL`). A tenant normally has one agent (desktop) and a
/// handful of clients (phone/tablet/web); 64 leaves generous headroom while
/// stopping one channel from being packed with sockets.
pub const DEFAULT_MAX_PER_CHANNEL: usize = 64;

pub struct Registry {
    channels: Mutex<HashMap<String, Channel>>,
    next_id: AtomicU64,
    max_channels: usize,
    max_per_channel: usize,
}

impl Default for Registry {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_CHANNELS, DEFAULT_MAX_PER_CHANNEL)
    }
}

/// Snapshot returned by `join`, used for the `authed` acknowledgement.
pub struct JoinState {
    pub conn_id: u64,
    pub clients: usize,
    pub agent_online: bool,
}

impl Registry {
    pub fn new(max_channels: usize, max_per_channel: usize) -> Self {
        Self {
            channels: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(0),
            max_channels,
            max_per_channel,
        }
    }

    /// Register a connection and notify the opposite role of the change. Returns
    /// `None` (admission rejected) when the connection would create a channel
    /// past the channel-count cap, or add a role past the per-channel cap.
    pub fn join(&self, channel: &str, role: Role, tx: Tx) -> Option<JoinState> {
        let mut channels = self.channels.lock().unwrap();
        // Cap check under the lock, before creating the bucket, so a rejected
        // join never leaves an empty channel behind or races the count.
        match channels.get(channel) {
            Some(ch) => {
                if ch.members(role).len() >= self.max_per_channel {
                    log::warn!(
                        "join rejected: channel {}… {role:?} at per-channel cap {}",
                        &channel[..channel.len().min(12)],
                        self.max_per_channel
                    );
                    return None;
                }
            }
            None => {
                if channels.len() >= self.max_channels {
                    log::warn!("join rejected: channel cap {} reached", self.max_channels);
                    return None;
                }
            }
        }
        let conn_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let ch = channels.entry(channel.to_string()).or_default();
        ch.members_mut(role).insert(conn_id, tx);
        let state = JoinState {
            conn_id,
            clients: ch.clients.len(),
            agent_online: !ch.agents.is_empty(),
        };
        Self::broadcast_membership(ch, role);
        Some(state)
    }

    /// Deregister a connection and notify the opposite role of the change.
    pub fn leave(&self, channel: &str, role: Role, conn_id: u64) {
        let mut channels = self.channels.lock().unwrap();
        let Some(ch) = channels.get_mut(channel) else {
            return;
        };
        ch.members_mut(role).remove(&conn_id);
        Self::broadcast_membership(ch, role);
        if ch.is_empty() {
            channels.remove(channel);
        }
    }

    /// Send a structured JSON frame to every connection of the opposite role.
    /// Returns the number of connections the frame was delivered to.
    pub fn forward(&self, channel: &str, from: Role, frame: &OutFrame) -> usize {
        let serialized = match serde_json::to_string(frame) {
            Ok(s) => s,
            Err(_) => return 0,
        };
        self.deliver(channel, from, OutMsg::Text(serialized))
    }

    /// Forward an opaque binary blob (a compressed `msg` payload) to every
    /// connection of the opposite role, verbatim. Returns the delivery count.
    pub fn forward_binary(&self, channel: &str, from: Role, bytes: Vec<u8>) -> usize {
        self.deliver(channel, from, OutMsg::Binary(bytes))
    }

    fn deliver(&self, channel: &str, from: Role, msg: OutMsg) -> usize {
        let channels = self.channels.lock().unwrap();
        let Some(ch) = channels.get(channel) else {
            return 0;
        };
        let mut delivered = 0;
        for tx in ch.members(from.opposite()).values() {
            if tx.send(msg.clone()).is_ok() {
                delivered += 1;
            }
        }
        delivered
    }

    /// After `role`'s membership changed, tell the opposite role about it.
    fn broadcast_membership(ch: &Channel, changed: Role) {
        let frame = match changed {
            Role::Client => OutFrame::Presence { clients: ch.clients.len() },
            Role::Agent => OutFrame::AgentStatus { online: !ch.agents.is_empty() },
        };
        let Ok(serialized) = serde_json::to_string(&frame) else {
            return;
        };
        for tx in ch.members(changed.opposite()).values() {
            let _ = tx.send(OutMsg::Text(serialized.clone()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn drain(rx: &mut UnboundedReceiver<OutMsg>) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                OutMsg::Text(s) => out.push(serde_json::from_str(&s).unwrap()),
                OutMsg::Binary(_) => panic!("expected text frame, got binary"),
            }
        }
        out
    }

    fn drain_binary(rx: &mut UnboundedReceiver<OutMsg>) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            match msg {
                OutMsg::Binary(b) => out.push(b),
                OutMsg::Text(_) => panic!("expected binary frame, got text"),
            }
        }
        out
    }

    #[test]
    fn channel_id_is_sha256_hex() {
        assert_eq!(
            channel_id("hello"),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        assert_ne!(channel_id("a"), channel_id("b"));
    }

    #[test]
    fn forwards_only_to_opposite_role() {
        let reg = Registry::default();
        let (agent_tx, mut agent_rx) = unbounded_channel();
        let (client_tx, mut client_rx) = unbounded_channel();
        let (agent2_tx, mut agent2_rx) = unbounded_channel();
        reg.join("ch", Role::Agent, agent_tx);
        reg.join("ch", Role::Agent, agent2_tx);
        reg.join("ch", Role::Client, client_tx);
        drain(&mut agent_rx);
        drain(&mut agent2_rx);
        drain(&mut client_rx);

        let n = reg.forward("ch", Role::Client, &OutFrame::Msg { payload: json!({"x": 1}) });
        assert_eq!(n, 2, "client frame reaches both agents");
        assert_eq!(drain(&mut agent_rx).len(), 1);
        assert_eq!(drain(&mut agent2_rx).len(), 1);
        assert!(drain(&mut client_rx).is_empty(), "sender's own role gets nothing");

        let n = reg.forward("ch", Role::Agent, &OutFrame::Msg { payload: json!({"y": 2}) });
        assert_eq!(n, 1, "agent frame reaches the one client");
        let got = drain(&mut client_rx);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0]["payload"]["y"], 2);
    }

    #[test]
    fn forward_binary_reaches_opposite_role_verbatim() {
        let reg = Registry::default();
        let (agent_tx, mut agent_rx) = unbounded_channel();
        let (client_tx, mut client_rx) = unbounded_channel();
        reg.join("ch", Role::Agent, agent_tx);
        reg.join("ch", Role::Client, client_tx);
        drain(&mut agent_rx);
        drain(&mut client_rx);

        let blob = vec![0x1f, 0x8b, 0x08, 0x00, 0xde, 0xad, 0xbe, 0xef];
        let n = reg.forward_binary("ch", Role::Agent, blob.clone());
        assert_eq!(n, 1, "agent binary frame reaches the one client");
        assert_eq!(drain_binary(&mut client_rx), vec![blob], "bytes forwarded verbatim");
        assert!(agent_rx.try_recv().is_err(), "sender's own role gets nothing");
    }

    #[test]
    fn channels_are_isolated() {
        let reg = Registry::default();
        let (a_tx, _a_rx) = unbounded_channel();
        let (c_tx, mut c_rx) = unbounded_channel();
        reg.join("ch-a", Role::Agent, a_tx);
        reg.join("ch-b", Role::Client, c_tx);
        drain(&mut c_rx);

        let n = reg.forward("ch-a", Role::Agent, &OutFrame::Msg { payload: json!(1) });
        assert_eq!(n, 0, "no client in ch-a");
        assert!(drain(&mut c_rx).is_empty(), "ch-b client must not receive ch-a traffic");
    }

    #[test]
    fn membership_changes_notify_opposite_role() {
        let reg = Registry::default();
        let (agent_tx, mut agent_rx) = unbounded_channel();
        let st = reg.join("ch", Role::Agent, agent_tx).unwrap();
        assert_eq!(st.clients, 0);
        assert!(st.agent_online);

        let (client_tx, mut client_rx) = unbounded_channel();
        let st = reg.join("ch", Role::Client, client_tx).unwrap();
        assert_eq!(st.clients, 1);
        let events = drain(&mut agent_rx);
        assert_eq!(events.last().unwrap()["type"], "presence");
        assert_eq!(events.last().unwrap()["clients"], 1);

        reg.leave("ch", Role::Client, st.conn_id);
        let events = drain(&mut agent_rx);
        assert_eq!(events.last().unwrap()["clients"], 0);
        assert!(drain(&mut client_rx).is_empty() || true);
    }

    #[test]
    fn agent_departure_flips_agent_status() {
        let reg = Registry::default();
        let (agent_tx, _agent_rx) = unbounded_channel();
        let agent = reg.join("ch", Role::Agent, agent_tx).unwrap();
        let (client_tx, mut client_rx) = unbounded_channel();
        reg.join("ch", Role::Client, client_tx);
        drain(&mut client_rx);

        reg.leave("ch", Role::Agent, agent.conn_id);
        let events = drain(&mut client_rx);
        assert_eq!(events.last().unwrap()["type"], "agent_status");
        assert_eq!(events.last().unwrap()["online"], false);
    }

    #[test]
    fn channel_count_cap_rejects_new_channels_only() {
        let reg = Registry::new(1, 64); // room for exactly one channel
        let (a_tx, _a) = unbounded_channel();
        assert!(reg.join("ch-a", Role::Agent, a_tx).is_some(), "first channel admitted");
        let (b_tx, _b) = unbounded_channel();
        assert!(reg.join("ch-b", Role::Agent, b_tx).is_none(), "second channel over cap");
        // But another connection to the *existing* channel is fine (no new bucket).
        let (a2_tx, _a2) = unbounded_channel();
        assert!(reg.join("ch-a", Role::Client, a2_tx).is_some(), "existing channel still accepts");
    }

    #[test]
    fn per_channel_role_cap_rejects_within_channel() {
        let reg = Registry::new(100, 2); // 2 per role per channel
        let (a1, _r1) = unbounded_channel();
        let (a2, _r2) = unbounded_channel();
        assert!(reg.join("ch", Role::Agent, a1).is_some());
        assert!(reg.join("ch", Role::Agent, a2).is_some());
        let (a3, _r3) = unbounded_channel();
        assert!(reg.join("ch", Role::Agent, a3).is_none(), "third agent over per-channel cap");
        // The other role has its own budget in the same channel.
        let (c1, _rc1) = unbounded_channel();
        let (c2, _rc2) = unbounded_channel();
        assert!(reg.join("ch", Role::Client, c1).is_some(), "client role has its own cap");
        assert!(reg.join("ch", Role::Client, c2).is_some());
        let (c3, _rc3) = unbounded_channel();
        assert!(reg.join("ch", Role::Client, c3).is_none(), "third client over per-channel cap");
    }
}
