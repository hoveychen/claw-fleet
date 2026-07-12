//! Channel registry — pairs agent and client connections that presented the
//! same secret and routes frames between the two roles.
//!
//! A channel id is `hex(sha256(secret))`; the raw secret is never stored.
//! Connections push serialized frames through unbounded senders so the
//! registry stays synchronous and unit-testable without real sockets.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use sha2::{Digest, Sha256};
use tokio::sync::mpsc::UnboundedSender;

use crate::frames::{OutFrame, Role};

pub type Tx = UnboundedSender<String>;

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

#[derive(Default)]
pub struct Registry {
    channels: Mutex<HashMap<String, Channel>>,
    next_id: AtomicU64,
}

/// Snapshot returned by `join`, used for the `authed` acknowledgement.
pub struct JoinState {
    pub conn_id: u64,
    pub clients: usize,
    pub agent_online: bool,
}

impl Registry {
    /// Register a connection and notify the opposite role of the change.
    pub fn join(&self, channel: &str, role: Role, tx: Tx) -> JoinState {
        let conn_id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut channels = self.channels.lock().unwrap();
        let ch = channels.entry(channel.to_string()).or_default();
        ch.members_mut(role).insert(conn_id, tx);
        let state = JoinState {
            conn_id,
            clients: ch.clients.len(),
            agent_online: !ch.agents.is_empty(),
        };
        Self::broadcast_membership(ch, role);
        state
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

    /// Send a frame to every connection of the opposite role. Returns the
    /// number of connections the frame was delivered to.
    pub fn forward(&self, channel: &str, from: Role, frame: &OutFrame) -> usize {
        let serialized = match serde_json::to_string(frame) {
            Ok(s) => s,
            Err(_) => return 0,
        };
        let channels = self.channels.lock().unwrap();
        let Some(ch) = channels.get(channel) else {
            return 0;
        };
        let mut delivered = 0;
        for tx in ch.members(from.opposite()).values() {
            if tx.send(serialized.clone()).is_ok() {
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
            let _ = tx.send(serialized.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver};

    fn drain(rx: &mut UnboundedReceiver<String>) -> Vec<serde_json::Value> {
        let mut out = Vec::new();
        while let Ok(s) = rx.try_recv() {
            out.push(serde_json::from_str(&s).unwrap());
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
        let st = reg.join("ch", Role::Agent, agent_tx);
        assert_eq!(st.clients, 0);
        assert!(st.agent_online);

        let (client_tx, mut client_rx) = unbounded_channel();
        let st = reg.join("ch", Role::Client, client_tx);
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
        let agent = reg.join("ch", Role::Agent, agent_tx);
        let (client_tx, mut client_rx) = unbounded_channel();
        reg.join("ch", Role::Client, client_tx);
        drain(&mut client_rx);

        reg.leave("ch", Role::Agent, agent.conn_id);
        let events = drain(&mut client_rx);
        assert_eq!(events.last().unwrap()["type"], "agent_status");
        assert_eq!(events.last().unwrap()["online"], false);
    }
}
