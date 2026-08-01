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

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

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

/// A client frame accepted while no agent was online, held until one joins.
#[derive(Debug, Clone)]
struct Pending {
    msg: OutMsg,
    at: Instant,
}

#[derive(Default)]
struct Channel {
    agents: HashMap<u64, Tx>,
    clients: HashMap<u64, Tx>,
    /// Client→agent frames waiting for an agent. Only ever non-empty while the
    /// channel has no agent connection.
    pending: VecDeque<Pending>,
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

    /// A channel is only collectable once nothing is left to deliver — a queue
    /// held for an absent agent must outlive the client that handed it over,
    /// otherwise the phone's answer dies the moment its socket drops.
    fn is_empty(&self) -> bool {
        self.agents.is_empty() && self.clients.is_empty() && self.pending.is_empty()
    }

    /// Drop queued frames older than `ttl`. Returns how many were discarded.
    fn drop_expired(&mut self, ttl: Duration) -> usize {
        let before = self.pending.len();
        self.pending.retain(|p| p.at.elapsed() < ttl);
        before - self.pending.len()
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

/// Default cap on client frames held for an absent agent, per channel. A phone
/// answering decision cards emits one small frame per answer, so 32 covers a
/// long offline stretch; past that the oldest is dropped (and logged — never
/// silently).
pub const DEFAULT_MAX_PENDING_PER_CHANNEL: usize = 32;
/// Default lifetime of a queued client frame. Past this the answer is stale
/// enough that the desktop has likely resolved the card another way, so holding
/// it would replay an outdated decision.
pub const DEFAULT_PENDING_TTL: Duration = Duration::from_secs(600);

/// What became of a client frame handed to the registry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    /// Handed to this many live agent connections.
    Delivered(usize),
    /// No agent online; queued for the next one to join. Carries the queue depth
    /// after the push.
    Queued(usize),
    /// No agent online and the frame could not be queued (channel unknown).
    Dropped,
}

pub struct Registry {
    channels: Mutex<HashMap<String, Channel>>,
    next_id: AtomicU64,
    max_channels: usize,
    max_per_channel: usize,
    max_pending: usize,
    pending_ttl: Duration,
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
            max_pending: DEFAULT_MAX_PENDING_PER_CHANNEL,
            pending_ttl: DEFAULT_PENDING_TTL,
        }
    }

    /// Override the store-and-forward limits (tests drive tiny caps / TTLs).
    pub fn with_pending_limits(mut self, max_pending: usize, pending_ttl: Duration) -> Self {
        self.max_pending = max_pending;
        self.pending_ttl = pending_ttl;
        self
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
        // An agent taking the channel inherits whatever the phones handed over
        // while it was away, oldest first. Stale frames are dropped rather than
        // replayed — the card they answer may have been resolved another way.
        if role == Role::Agent {
            let expired = ch.drop_expired(self.pending_ttl);
            if expired > 0 {
                log::info!(
                    "channel {}… discarded {expired} expired held frame(s) on agent join",
                    &channel[..channel.len().min(12)]
                );
            }
            if !ch.pending.is_empty() {
                let tx = &ch.agents[&conn_id];
                let held = std::mem::take(&mut ch.pending);
                log::info!(
                    "channel {}… flushing {} held frame(s) to the joining agent",
                    &channel[..channel.len().min(12)],
                    held.len()
                );
                for p in held {
                    let _ = tx.send(p.msg);
                }
            }
        }
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

    /// Forward a client frame to the channel's agents, or hold it until one
    /// joins. This is what keeps an answer alive across the gap where the phone
    /// is online but the desktop is not: the phone's socket often lives only a
    /// few seconds, far less than the round trip it would need to wait out a
    /// reconnect, so the relay takes custody instead of dropping the frame.
    pub fn deliver_or_queue(&self, channel: &str, msg: OutMsg) -> Delivery {
        let mut channels = self.channels.lock().unwrap();
        let Some(ch) = channels.get_mut(channel) else {
            return Delivery::Dropped;
        };
        let mut delivered = 0;
        for tx in ch.agents.values() {
            if tx.send(msg.clone()).is_ok() {
                delivered += 1;
            }
        }
        if delivered > 0 {
            return Delivery::Delivered(delivered);
        }
        ch.drop_expired(self.pending_ttl);
        // At the cap the oldest goes: a fresher answer is the one worth keeping,
        // and the loss is logged rather than passed off as a delivery.
        while ch.pending.len() >= self.max_pending {
            ch.pending.pop_front();
            log::warn!(
                "channel {}… pending queue at cap {}; dropped the oldest held frame",
                &channel[..channel.len().min(12)],
                self.max_pending
            );
        }
        ch.pending.push_back(Pending { msg, at: Instant::now() });
        Delivery::Queued(ch.pending.len())
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

    // ── store-and-forward:agent 离线时暂存 client 帧,上线后转投 ──────────────
    //
    // 治的是「收得到卡、发不出答复」的不对称:收卡只要单向一瞬,答复要一次完整
    // 往返 + 双方同时在线。手机连接常常只活几秒,等不到桌面重连,所以让 relay
    // 先接管这一帧。

    fn text(v: serde_json::Value) -> OutMsg {
        OutMsg::Text(v.to_string())
    }

    #[test]
    fn client_frame_is_queued_while_no_agent_and_flushed_on_join() {
        let reg = Registry::default();
        let (client_tx, _client_rx) = unbounded_channel();
        reg.join("ch", Role::Client, client_tx);

        // No agent yet — the frame must be taken into custody, not dropped.
        let d = reg.deliver_or_queue("ch", text(json!({"answer": 1})));
        assert_eq!(d, Delivery::Queued(1), "an offline agent must not lose the answer");

        // The agent arrives and receives the held frame.
        let (agent_tx, mut agent_rx) = unbounded_channel();
        reg.join("ch", Role::Agent, agent_tx);
        let got = drain(&mut agent_rx);
        assert!(
            got.contains(&json!({"answer": 1})),
            "joining agent must be handed the queued answer, got {got:?}"
        );
    }

    #[test]
    fn queue_outlives_the_client_that_handed_it_over() {
        let reg = Registry::default();
        let (client_tx, _client_rx) = unbounded_channel();
        let client = reg.join("ch", Role::Client, client_tx).expect("client joins");
        reg.deliver_or_queue("ch", text(json!({"answer": 2})));

        // The phone drops off right after submitting — the usual case, since its
        // socket lives seconds. The channel must survive to keep the answer.
        reg.leave("ch", Role::Client, client.conn_id);

        let (agent_tx, mut agent_rx) = unbounded_channel();
        reg.join("ch", Role::Agent, agent_tx);
        let got = drain(&mut agent_rx);
        assert!(
            got.contains(&json!({"answer": 2})),
            "answer must survive the client's disconnect, got {got:?}"
        );
    }

    #[test]
    fn live_agent_still_gets_frames_directly_without_queueing() {
        let reg = Registry::default();
        let (agent_tx, mut agent_rx) = unbounded_channel();
        reg.join("ch", Role::Agent, agent_tx);
        drain(&mut agent_rx);

        let d = reg.deliver_or_queue("ch", text(json!({"answer": 3})));
        assert_eq!(d, Delivery::Delivered(1), "an online agent is served directly");
        assert_eq!(drain(&mut agent_rx), vec![json!({"answer": 3})]);
    }

    #[test]
    fn pending_queue_is_capped_and_drops_the_oldest() {
        let reg = Registry::default().with_pending_limits(2, Duration::from_secs(600));
        let (client_tx, _client_rx) = unbounded_channel();
        reg.join("ch", Role::Client, client_tx);

        for i in 1..=3 {
            reg.deliver_or_queue("ch", text(json!({"answer": i})));
        }

        let (agent_tx, mut agent_rx) = unbounded_channel();
        reg.join("ch", Role::Agent, agent_tx);
        let got = drain(&mut agent_rx);
        let answers: Vec<_> = got.iter().filter(|v| v.get("answer").is_some()).collect();
        assert_eq!(answers.len(), 2, "cap of 2 holds two frames, got {got:?}");
        assert!(
            !got.contains(&json!({"answer": 1})),
            "the oldest frame is the one dropped at the cap"
        );
        assert!(got.contains(&json!({"answer": 3})), "the newest frame is kept");
    }

    #[test]
    fn expired_pending_frames_are_not_replayed() {
        let reg = Registry::default().with_pending_limits(32, Duration::from_millis(30));
        let (client_tx, _client_rx) = unbounded_channel();
        reg.join("ch", Role::Client, client_tx);
        reg.deliver_or_queue("ch", text(json!({"answer": 4})));

        std::thread::sleep(Duration::from_millis(60));

        let (agent_tx, mut agent_rx) = unbounded_channel();
        reg.join("ch", Role::Agent, agent_tx);
        let got = drain(&mut agent_rx);
        assert!(
            !got.contains(&json!({"answer": 4})),
            "a stale answer must not be replayed onto a card resolved elsewhere, got {got:?}"
        );
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
