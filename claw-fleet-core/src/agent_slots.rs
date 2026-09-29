//! Process-wide cap on concurrently running agent processes.
//!
//! A small cloud container runs every customer turn as its own `claude -p`
//! process, each a few hundred MB resident. Four at once filled a 1.2 GB cgroup
//! with anonymous memory; the kernel then evicted the binaries' own code pages
//! and re-read them from disk on every fault, pinning the host's disk at
//! 150 MB/s and stalling every other tenant on it. No OOM kill ever fired, so
//! nothing broke loudly — everything just crawled.
//!
//! `FLEET_MAX_CONCURRENT_AGENTS` bounds that. Unset, `0` or unparseable means
//! no cap, which is exactly the old behaviour.
//!
//! Counting happens at the one chokepoint every claude launch passes through
//! ([`crate::session_launch::spawn_claude_detached_with_envs`]), so admin
//! spawns, auto-resume and handoffs are all *counted*. Only callers that can
//! afford to wait call [`admit`] — the ACP turn thread, which is dedicated to
//! one prompt anyway. An HTTP worker must not park for minutes, so the admin
//! routes are counted but never queued.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::Duration;

const ENV: &str = "FLEET_MAX_CONCURRENT_AGENTS";

/// How often a queued turn re-checks for a free slot (and for cancellation).
const POLL: Duration = Duration::from_millis(500);

static LIVE: AtomicUsize = AtomicUsize::new(0);

/// Serialises check-then-spawn. Held from "a slot is free" until the spawn has
/// incremented [`LIVE`], so two queued turns cannot both see the last free slot.
static ADMIT: Mutex<()> = Mutex::new(());

/// The configured cap, or `None` for unlimited.
pub fn limit() -> Option<usize> {
    parse_limit(std::env::var(ENV).ok().as_deref())
}

fn parse_limit(raw: Option<&str>) -> Option<usize> {
    raw.and_then(|v| v.trim().parse::<usize>().ok())
        .filter(|&n| n > 0)
}

/// Agent processes currently alive (spawned and not yet reaped).
pub fn live() -> usize {
    LIVE.load(Ordering::SeqCst)
}

/// Marks one agent process alive until dropped. Taken right after a successful
/// spawn and dropped by the reaper once the child has exited.
pub struct Slot(());

impl Drop for Slot {
    fn drop(&mut self) {
        LIVE.fetch_sub(1, Ordering::SeqCst);
    }
}

pub(crate) fn track() -> Slot {
    LIVE.fetch_add(1, Ordering::SeqCst);
    Slot(())
}

/// Wait until fewer than the configured cap are alive.
///
/// Returns the admission guard — keep it until the spawn call has returned, so
/// the new process is counted before the next waiter looks. `Err` means
/// `cancelled` fired while queued. `on_wait` runs once, only if the call
/// actually has to wait.
pub fn admit(
    cancelled: impl Fn() -> bool,
    on_wait: impl FnOnce(usize),
) -> Result<MutexGuard<'static, ()>, ()> {
    admit_with(limit(), cancelled, on_wait)
}

fn admit_with(
    limit: Option<usize>,
    cancelled: impl Fn() -> bool,
    on_wait: impl FnOnce(usize),
) -> Result<MutexGuard<'static, ()>, ()> {
    let mut on_wait = Some(on_wait);
    loop {
        // try_lock, not lock: a queued turn must stay able to notice its own
        // cancellation instead of blocking behind whoever holds the gate.
        if let Ok(guard) = ADMIT.try_lock() {
            let busy = live();
            match limit {
                Some(cap) if busy >= cap => {
                    drop(guard);
                    if let Some(f) = on_wait.take() {
                        f(busy);
                    }
                }
                _ => return Ok(guard),
            }
        }
        if cancelled() {
            return Err(());
        }
        std::thread::sleep(POLL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    /// LIVE and ADMIT are process globals; tests that move them must not overlap.
    fn serial() -> MutexGuard<'static, ()> {
        static LOCK: Mutex<()> = Mutex::new(());
        LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn unset_zero_and_garbage_mean_unlimited() {
        assert_eq!(parse_limit(None), None);
        assert_eq!(parse_limit(Some("0")), None);
        assert_eq!(parse_limit(Some("two")), None);
        assert_eq!(parse_limit(Some(" 2 ")), Some(2));
    }

    #[test]
    fn a_slot_counts_until_dropped() {
        let _s = serial();
        let base = live();
        let slot = track();
        assert_eq!(live(), base + 1);
        drop(slot);
        assert_eq!(live(), base);
    }

    #[test]
    fn unlimited_admits_without_waiting() {
        let _s = serial();
        let _held = track();
        let waited = AtomicBool::new(false);
        let g = admit_with(None, || false, |_| waited.store(true, Ordering::SeqCst));
        assert!(g.is_ok());
        assert!(!waited.load(Ordering::SeqCst));
    }

    #[test]
    fn a_full_cap_waits_until_a_slot_frees() {
        let _s = serial();
        let cap = live() + 1;
        let held = track();
        let releaser = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(700));
            drop(held);
        });
        let waited_with = Mutex::new(None);
        let g = admit_with(Some(cap), || false, |n| {
            *waited_with.lock().unwrap() = Some(n)
        });
        assert!(g.is_ok());
        assert_eq!(*waited_with.lock().unwrap(), Some(cap));
        releaser.join().unwrap();
    }

    #[test]
    fn a_queued_turn_gives_up_when_cancelled() {
        let _s = serial();
        let cap = live() + 1;
        let _held = track();
        let polls = AtomicUsize::new(0);
        let g = admit_with(
            Some(cap),
            || polls.fetch_add(1, Ordering::SeqCst) >= 1,
            |_| {},
        );
        assert!(g.is_err());
    }
}
