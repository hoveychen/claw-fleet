//! Timing probe for the detail pane's Tauri commands.
//!
//! # Why this exists
//!
//! 「对话」Tab sat on 「加载中…」 forever against a dsh session, and every layer
//! underneath measured healthy: `dsh session.history` answered a 150-message
//! tail in 0.03s, and `fleet serve`'s `/messages?path=dsh://…&tail=150` — the
//! same `LocalBackend::get_messages_tail` the desktop calls — returned 122
//! messages in 0.47s. Driving the real frontend against that probe rendered the
//! conversation in 918ms. So the failure lived in the desktop process, and the
//! probe is what separates "the call was slow" from "the call never returned"
//! there — one log line per slow command, with the outcome attached.
//!
//! (When it was written the desktop still held its backend behind an `RwLock`
//! and the probe also split lock-wait from call time; the lock is gone, so the
//! whole span is the call.)
//!
//! # Why some probes need a watchdog
//!
//! A command that never returns never reaches its completion log, so
//! completion-based logging is blind to exactly the failure being chased.
//! [`CmdProbe::start_watched`] arms a thread that reports while the call is
//! still outstanding. It is reserved for the commands that fire once per
//! session open; the polled read commands log on completion only, so an
//! active session does not spend a thread every 700ms.

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

/// Stopwatch for one detail-pane Tauri command.
pub(crate) struct CmdProbe {
    label: &'static str,
    detail: String,
    started: Instant,
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

    /// Close the probe, logging the duration when the total was slow.
    ///
    /// `outcome` is whatever identifies the result at a glance (a row count, an
    /// error). It is only read on the slow path.
    pub(crate) fn done(self, outcome: impl FnOnce() -> String) {
        self.finished.store(true, Ordering::SeqCst);
        let total = self.started.elapsed();
        if total.as_millis() < SLOW_MS {
            return;
        }
        claw_fleet_core::log_debug(&format!(
            "cmd probe: {} took {}ms — {} [{}]",
            self.label,
            total.as_millis(),
            outcome(),
            self.detail,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `done` only logs, so the observable contract is that closing a probe is
    /// well-defined rather than a panic — on the fast path and the slow one.
    #[test]
    fn closing_a_probe_is_well_defined() {
        let probe = CmdProbe::start("get_messages_tail", "dsh://session-x");
        assert!(probe.started.elapsed() < Duration::from_secs(1));
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
