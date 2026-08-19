//! Timing probe for the detail pane's Tauri commands.
//!
//! # Why this exists
//!
//! 「对话」Tab sat on 「加载中…」 forever against a dsh session, and every layer
//! underneath measured healthy: `dsh session.history` answered a 150-message
//! tail in 0.03s, and `fleet serve`'s `/messages?path=dsh://…&tail=150` — the
//! same `Backend::get_messages_tail` the desktop calls — returned 122 messages
//! in 0.47s. Driving the real frontend against that probe rendered the
//! conversation in 918ms. So the failure lives in the desktop process, and the
//! only thing there that the HTTP path does not have is [`AppState::backend`]'s
//! `RwLock`: `get_messages_tail` / `read_live_thinking` take it for **read**
//! (1.5s / 700ms polls while a session is active), `start_watching_session` /
//! `stop_watching_session` take it for **write**.
//!
//! One log line therefore has to separate *waiting for the lock* from *running
//! under it* — a multi-second total with a fast call means the queue, not dsh.
//!
//! # Why some probes need a watchdog
//!
//! A command that never returns never reaches its completion log, so
//! completion-based logging is blind to exactly the failure being chased.
//! [`CmdProbe::start_watched`] arms a thread that reports while the call is
//! still outstanding. It is reserved for the write-lock commands, which fire
//! once per session open; the polled read commands log on completion only, so
//! an active session does not spend a thread every 700ms.
//!
//! [`AppState::backend`]: crate::AppState::backend

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// Total duration that makes a detail-pane command worth a log line.
///
/// Sized above the slowest healthy call measured on this path (0.47s for a dsh
/// `get_messages_tail` through the probe) so a normal active session does not
/// write a line every 1.5s.
const SLOW_MS: u128 = 1_000;

/// Points at which a watched command reports that it is *still* waiting.
const WATCHDOG_MARKS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(15),
];

/// Stopwatch for one Tauri command that goes through the backend lock.
pub(crate) struct CmdProbe {
    label: &'static str,
    detail: String,
    started: Instant,
    /// When the backend lock came in hand; `None` until [`Self::locked`].
    locked_at: Option<Instant>,
    /// Set on drop/completion so an armed watchdog stops reporting.
    finished: Arc<AtomicBool>,
}

impl CmdProbe {
    /// Probe that logs only if the whole call turns out slow.
    pub(crate) fn start(label: &'static str, detail: impl Into<String>) -> Self {
        Self {
            label,
            detail: detail.into(),
            started: Instant::now(),
            locked_at: None,
            finished: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Probe that also reports *while* it is outstanding — see the module note.
    pub(crate) fn start_watched(label: &'static str, detail: impl Into<String>) -> Self {
        let probe = Self::start(label, detail);
        let finished = probe.finished.clone();
        let label = probe.label;
        let detail = probe.detail.clone();
        // Best-effort: if the thread cannot be spawned we simply lose the
        // in-flight reports, and the completion log still lands.
        let _ = std::thread::Builder::new()
            .name("cmd-probe-watchdog".into())
            .spawn(move || {
                let mut waited = Duration::ZERO;
                for mark in WATCHDOG_MARKS {
                    std::thread::sleep(mark - waited);
                    waited = mark;
                    if finished.load(Ordering::SeqCst) {
                        return;
                    }
                    claw_fleet_core::log_debug(&format!(
                        "cmd probe: {label} still outstanding after {}s ({detail})",
                        waited.as_secs()
                    ));
                }
            });
        probe
    }

    /// Record that the backend lock is in hand. Everything before this point was
    /// queueing; everything after is the call itself.
    pub(crate) fn locked(&mut self) {
        self.locked_at = Some(Instant::now());
    }

    /// Close the probe, logging the wait/call split when the total was slow.
    ///
    /// `outcome` is whatever identifies the result at a glance (a row count, an
    /// error). It is only read on the slow path.
    pub(crate) fn done(self, outcome: impl FnOnce() -> String) {
        self.finished.store(true, Ordering::SeqCst);
        let total = self.started.elapsed();
        if total.as_millis() < SLOW_MS {
            return;
        }
        let (wait_ms, call_ms) = match self.locked_at {
            Some(at) => (
                (at - self.started).as_millis(),
                at.elapsed().as_millis(),
            ),
            // Never acquired the lock — the whole time was the wait.
            None => (total.as_millis(), 0),
        };
        claw_fleet_core::log_debug(&format!(
            "cmd probe: {} took {}ms (lock wait {}ms + call {}ms) — {} [{}]",
            self.label,
            total.as_millis(),
            wait_ms,
            call_ms,
            outcome(),
            self.detail,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The split is what makes a log line diagnostic: the same 「slow」 total
    /// means dsh when it is all `call`, and the lock queue when it is all
    /// `wait`. A probe that never got the lock must report the whole span as
    /// wait rather than silently attributing it to the call.
    #[test]
    fn a_probe_that_never_locked_reports_no_call_time() {
        let probe = CmdProbe::start("get_messages_tail", "dsh://session-x");
        assert!(probe.locked_at.is_none());
        // `done` only logs, so the observable contract here is that closing an
        // unlocked probe is well-defined rather than a panic.
        probe.done(|| "abandoned".into());
    }

    #[test]
    fn locking_splits_the_span() {
        let mut probe = CmdProbe::start("get_messages_tail", "dsh://session-x");
        std::thread::sleep(Duration::from_millis(5));
        probe.locked();
        let locked_at = probe.locked_at.expect("just set");
        assert!(locked_at > probe.started);
        probe.done(|| "1 msg".into());
    }

    /// A finished probe must silence its watchdog — otherwise every session open
    /// would keep a thread logging for 15s.
    #[test]
    fn finishing_silences_the_watchdog() {
        let probe = CmdProbe::start_watched("start_watching_session", "dsh://session-x");
        let flag = probe.finished.clone();
        assert!(!flag.load(Ordering::SeqCst));
        probe.done(|| "ok".into());
        assert!(flag.load(Ordering::SeqCst));
    }
}
