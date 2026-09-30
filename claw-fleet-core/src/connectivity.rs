//! Is the machine online right now? Consulted before Fleet spends something a
//! network outage would waste: a server-error retry's budget, or a plan
//! reviver's orphan clock and spawn.
//!
//! Why: on a MacBook asleep on battery, each DarkWake let the server-error
//! retry fire and die on `getaddrinfo ENOTFOUND api.anthropic.com` (session
//! 765f46e9, at least 5 times in one night), burning the whole per-episode
//! budget before the lid opened. The same outage kept the reviver's 30-minute
//! orphan clock running, so the first tick after waking revived a plan whose
//! owner had only been waiting for the network (semgap-recount, 2026-09-25).
//!
//! The probe is the failure itself: resolve the API host. It is cached for
//! [`CACHE_MS`] and only run when a caller is about to act, so an idle machine
//! does no lookups. Transitions are logged and written to
//! `~/.fleet/connectivity.json` so "why did nothing retry" has an answer on
//! disk.

use std::net::ToSocketAddrs;
use std::sync::Mutex;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Host whose resolution stands in for "online". The retries this gates are
/// Claude Code sessions talking to it.
const PROBE_HOST: &str = "api.anthropic.com:443";
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// How long one probe result is trusted.
pub const CACHE_MS: u64 = 20_000;

/// What the last probe found, as written to `~/.fleet/connectivity.json`.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Connectivity {
    pub online: bool,
    /// When the current state (online or offline) was first seen.
    pub since_ms: u64,
    /// When the probe last ran.
    pub checked_ms: u64,
}

static LAST: Mutex<Option<Connectivity>> = Mutex::new(None);

/// Whether the machine can reach the API host, probing at most every
/// [`CACHE_MS`]. Unit tests always see "online" and never touch the network.
pub fn is_online() -> bool {
    if cfg!(test) {
        return true;
    }
    let now = crate::plan_snooze::now_ms();
    let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(c) = last.as_ref() {
        if now.saturating_sub(c.checked_ms) < CACHE_MS {
            return c.online;
        }
    }
    let online = probe();
    let (next, changed) = observe(last.as_ref(), online, now);
    if changed {
        crate::log_debug(&format!(
            "connectivity: {} (probe {PROBE_HOST})",
            if online { "online" } else { "offline" }
        ));
        write_state(&next);
    }
    *last = Some(next);
    online
}

/// Fold one probe result into the previous state. Returns the new state and
/// whether online/offline flipped (or this is the first observation).
fn observe(prev: Option<&Connectivity>, online: bool, now: u64) -> (Connectivity, bool) {
    match prev {
        Some(p) if p.online == online => {
            (Connectivity { online, since_ms: p.since_ms, checked_ms: now }, false)
        }
        _ => (Connectivity { online, since_ms: now, checked_ms: now }, true),
    }
}

/// Resolve [`PROBE_HOST`] on a helper thread, giving up after
/// [`PROBE_TIMEOUT`]: `getaddrinfo` has no timeout of its own and can hang for
/// tens of seconds on a half-up network. A lookup that times out counts as
/// offline; its thread is left to finish on its own.
fn probe() -> bool {
    resolves(PROBE_HOST)
}

fn resolves(host: &'static str) -> bool {
    let (tx, rx) = std::sync::mpsc::channel();
    let spawned = std::thread::Builder::new().name("connectivity-probe".into()).spawn(move || {
        let ok = host.to_socket_addrs().is_ok_and(|mut a| a.next().is_some());
        let _ = tx.send(ok);
    });
    if spawned.is_err() {
        // Cannot tell; do not block the work this gates.
        return true;
    }
    rx.recv_timeout(PROBE_TIMEOUT).unwrap_or(false)
}

fn write_state(c: &Connectivity) {
    let Some(dir) = crate::session::get_fleet_dir() else { return };
    let Ok(json) = serde_json::to_vec_pretty(c) else { return };
    if let Err(e) = crate::atomic_json::write_atomic(&dir.join("connectivity.json"), &json) {
        crate::log_debug(&format!("connectivity: write state: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_keeps_its_start_until_it_flips() {
        let (first, changed) = observe(None, false, 100);
        assert!(changed);
        assert_eq!(first, Connectivity { online: false, since_ms: 100, checked_ms: 100 });
        let (still, changed) = observe(Some(&first), false, 200);
        assert!(!changed);
        assert_eq!(still, Connectivity { online: false, since_ms: 100, checked_ms: 200 });
        let (back, changed) = observe(Some(&still), true, 300);
        assert!(changed);
        assert_eq!(back, Connectivity { online: true, since_ms: 300, checked_ms: 300 });
    }

    /// Live check of the real probe: `cargo test connectivity -- --ignored`.
    #[test]
    #[ignore]
    fn probe_resolves_the_api_host_live() {
        assert!(probe());
        // What a DarkWake without network looks like to the probe.
        assert!(!resolves("fleet-probe.invalid:443"));
    }

    #[test]
    fn tests_never_probe_the_network() {
        assert!(is_online());
    }
}
