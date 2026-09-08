//! Heartbeat on the main thread, so "the app froze for a moment" stops being
//! a feeling.
//!
//! # Why
//!
//! Tauri runs a plain `#[tauri::command] fn` **inline on the event loop**, and
//! the event loop is also what delivers every invoke *response* into the
//! webview, what `emit()` ends in (a webview eval), and what macOS presents
//! native panels on. So one slow sync command does not merely delay itself: it
//! stalls unrelated invokes that had already finished their work and it keeps a
//! native save dialog from ever appearing. Both were observed on 2026-09-08 —
//! see wiki `desktop/ipc-stall-forensics`.
//!
//! The existing `cmd_probe` cannot see that class of failure: it times the
//! commands it wraps (7 of 250+), and a *victim* command is fast by its own
//! clock — it is the answer's ride home that is late.
//!
//! # What it measures, and what it deliberately does not
//!
//! One round trip: dispatch a do-nothing closure to the main thread, and see
//! how long it takes to run. Nothing else is inside the measured span — in
//! particular the log write happens on this thread *after* the measurement, so
//! a slow disk cannot be misread as a busy main thread. (That inversion has
//! already happened once here: an earlier watchdog reported "thread stalled"
//! for what turned out to be file-write latency.)

use std::sync::mpsc;
use std::time::{Duration, Instant};

/// How often to take a reading. Cheap enough to be continuous: an empty
/// closure on the event loop is nothing next to the paint it sits beside.
const INTERVAL: Duration = Duration::from_millis(500);

/// Round trips slower than this are worth a line. A healthy loop answers in
/// microseconds; a frame's worth of delay is normal under paint, so the
/// threshold is well above that and below anything a user would notice.
const STALL_MS: u128 = 1_000;

/// Give up on a single reading here and report it as still-blocked rather than
/// waiting out an arbitrarily long freeze in silence.
const RECV_TIMEOUT: Duration = Duration::from_secs(30);

/// The line to log for a reading, or `None` when the loop was responsive.
///
/// Split from the loop so the threshold is testable without an event loop.
fn stall_line(latency_ms: u128) -> Option<String> {
    (latency_ms >= STALL_MS).then(|| {
        format!("main-thread probe: event loop answered after {latency_ms}ms")
    })
}

/// Start the heartbeat. Runs until the app handle stops accepting work.
pub(crate) fn spawn(app: tauri::AppHandle) {
    let _ = std::thread::Builder::new()
        .name("main-thread-probe".into())
        .spawn(move || loop {
            std::thread::sleep(INTERVAL);
            let (tx, rx) = mpsc::channel();
            let started = Instant::now();
            // Err means the app is on its way out; stop rather than spin.
            if app.run_on_main_thread(move || { let _ = tx.send(()); }).is_err() {
                return;
            }
            match rx.recv_timeout(RECV_TIMEOUT) {
                Ok(()) => {
                    if let Some(line) = stall_line(started.elapsed().as_millis()) {
                        claw_fleet_core::log_debug(&line);
                    }
                }
                Err(_) => claw_fleet_core::log_debug(&format!(
                    "main-thread probe: event loop still blocked after {}s",
                    RECV_TIMEOUT.as_secs()
                )),
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A responsive loop must stay silent — otherwise the log fills with a line
    /// every 500ms and the real stalls become unfindable.
    #[test]
    fn a_responsive_loop_logs_nothing() {
        assert_eq!(stall_line(0), None);
        assert_eq!(stall_line(STALL_MS as u128 - 1), None);
    }

    /// And a stall reports the measured latency, since "how long" is the whole
    /// point of the reading.
    #[test]
    fn a_stall_reports_its_latency() {
        let line = stall_line(4_200).expect("4.2s must be reported");
        assert!(line.contains("4200ms"), "{line}");
    }
}
