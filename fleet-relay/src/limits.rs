//! Connection-admission caps for a public multi-tenant relay.
//!
//! The relay is a blind forwarder with no per-account authorization ("hold the
//! channel token, you're in"), so it needs coarse abuse backstops at the
//! connection layer: a global ceiling on concurrent sockets (memory) and a
//! per-source-IP ceiling (one actor can't hog the pool). These are *backstops*,
//! deliberately generous — a connection *flood* (churny connect/disconnect) is
//! better caught by the per-IP rate limiter (see `ratelimit.rs`); this module
//! only bounds how many sockets may be *held open* at once.
//!
//! Note on the per-IP key: behind carrier-grade NAT many unrelated users share
//! one public IP, so the per-IP ceiling is set high enough not to lock out a
//! whole CGNAT egress or a busy office, while still stopping a single host from
//! opening thousands of sockets.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// Default global concurrent-connection ceiling (override: `RELAY_MAX_CONNECTIONS`).
pub const DEFAULT_MAX_TOTAL: usize = 20_000;
/// Default per-IP concurrent-connection ceiling (override:
/// `RELAY_MAX_CONNECTIONS_PER_IP`). Generous on purpose — see the CGNAT note above.
pub const DEFAULT_MAX_PER_IP: usize = 200;

/// Tracks live connection counts and hands out RAII slots.
pub struct ConnLimiter {
    total: AtomicUsize,
    per_ip: Mutex<HashMap<IpAddr, usize>>,
    max_total: usize,
    max_per_ip: usize,
}

impl ConnLimiter {
    pub fn new(max_total: usize, max_per_ip: usize) -> Arc<Self> {
        Arc::new(Self {
            total: AtomicUsize::new(0),
            per_ip: Mutex::new(HashMap::new()),
            max_total,
            max_per_ip,
        })
    }

    /// Reserve a connection slot. Returns a guard that releases the slot when
    /// dropped, or `None` if the global or per-IP ceiling is already reached.
    ///
    /// `ip` is `None` when the client address can't be resolved (a direct/dev
    /// connection with no forwarding header); such connections still count
    /// toward the global ceiling but bypass the per-IP one (there's no key).
    pub fn try_acquire(self: &Arc<Self>, ip: Option<IpAddr>) -> Option<ConnGuard> {
        // Claim the global slot first with a CAS loop so a burst of concurrent
        // acquires can never push `total` past the ceiling.
        let mut cur = self.total.load(Ordering::Relaxed);
        loop {
            if cur >= self.max_total {
                return None;
            }
            match self.total.compare_exchange_weak(
                cur,
                cur + 1,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(actual) => cur = actual,
            }
        }
        // Then the per-IP slot; roll back the global claim if it's full.
        if let Some(ip) = ip {
            let mut map = self.per_ip.lock().unwrap();
            let n = map.entry(ip).or_insert(0);
            if *n >= self.max_per_ip {
                drop(map);
                self.total.fetch_sub(1, Ordering::AcqRel);
                return None;
            }
            *n += 1;
        }
        Some(ConnGuard { limiter: Arc::clone(self), ip })
    }

    #[cfg(test)]
    fn live_total(&self) -> usize {
        self.total.load(Ordering::Relaxed)
    }
}

/// Releases its reserved connection slot (global, and per-IP if keyed) on drop.
pub struct ConnGuard {
    limiter: Arc<ConnLimiter>,
    ip: Option<IpAddr>,
}

impl Drop for ConnGuard {
    fn drop(&mut self) {
        self.limiter.total.fetch_sub(1, Ordering::AcqRel);
        if let Some(ip) = self.ip {
            let mut map = self.limiter.per_ip.lock().unwrap();
            if let Some(n) = map.get_mut(&ip) {
                *n -= 1;
                if *n == 0 {
                    map.remove(&ip);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> Option<IpAddr> {
        Some(s.parse().unwrap())
    }

    #[test]
    fn per_ip_cap_rejects_beyond_limit() {
        let lim = ConnLimiter::new(100, 2);
        let a = lim.try_acquire(ip("1.2.3.4"));
        let b = lim.try_acquire(ip("1.2.3.4"));
        assert!(a.is_some() && b.is_some(), "first two from an IP admitted");
        assert!(lim.try_acquire(ip("1.2.3.4")).is_none(), "third from same IP rejected");
        // A different IP is unaffected.
        assert!(lim.try_acquire(ip("5.6.7.8")).is_some(), "other IP still admitted");
    }

    #[test]
    fn dropping_a_guard_frees_the_slot() {
        let lim = ConnLimiter::new(100, 1);
        let g = lim.try_acquire(ip("1.2.3.4"));
        assert!(g.is_some());
        assert!(lim.try_acquire(ip("1.2.3.4")).is_none(), "at per-IP cap");
        drop(g);
        assert!(lim.try_acquire(ip("1.2.3.4")).is_some(), "slot freed after drop");
    }

    #[test]
    fn global_cap_rejects_and_rolls_back_cleanly() {
        let lim = ConnLimiter::new(2, 100);
        let a = lim.try_acquire(ip("1.1.1.1"));
        let b = lim.try_acquire(ip("2.2.2.2"));
        assert!(a.is_some() && b.is_some());
        assert!(lim.try_acquire(ip("3.3.3.3")).is_none(), "global cap hit");
        assert_eq!(lim.live_total(), 2, "rejected acquire left no leaked global slot");
    }

    #[test]
    fn unkeyed_connections_count_globally_but_skip_per_ip() {
        let lim = ConnLimiter::new(100, 1);
        // No IP → per-IP cap of 1 doesn't apply; both admitted.
        let a = lim.try_acquire(None);
        let b = lim.try_acquire(None);
        assert!(a.is_some() && b.is_some(), "unkeyed connections bypass per-IP cap");
        assert_eq!(lim.live_total(), 2, "but they still count toward the global total");
    }
}
