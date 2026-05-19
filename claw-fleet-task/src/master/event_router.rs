//! Translate supervisor / scheduler / worker events into `append-user-message`
//! payloads for the master Claude Code session.
//!
//! Per PRD §5.7 / TASKS P21, every external signal that the master needs to
//! see arrives as a user-message append with a prefix that lets the master
//! filter by source:
//!
//! - `[event] ...` — system events (worker done / fail, scheduler update,
//!   touches hook triggered).
//! - `[user] ...` — user-originated text (mid-task append, AskUserQuestion
//!   reply, etc.).
//!
//! This module is **pure data**. It produces the message strings and
//! provides a 1-second debouncer for "scheduler dispatchable set changed"
//! events so the master doesn't get DoS'd by churn. The actual append
//! mechanism (writing into Claude Code's session input) is in the
//! subprocess integration layer — `crate::supervisor` once P7 lands.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use crate::pitem::{FailReason, PItemId};

/// A semantic event the master needs to see. Concrete variants map to
/// specific `[event] ...` formats; `Custom` is the escape hatch for things
/// that don't fit a category yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MasterEvent {
    /// Worker finished a P-item successfully (worker self-report — master
    /// still must run acceptance audit before mark-done).
    WorkerCompleted {
        p_item_id: PItemId,
        output_path: Option<String>,
    },
    /// Worker exited non-zero or self-reported failure.
    WorkerFailed {
        p_item_id: PItemId,
        reason: FailReason,
    },
    /// Scheduler's dispatchable set changed. Carries the new set so the
    /// master doesn't need a follow-up `get-dispatchable` call for this
    /// tick (it still may for fresh data).
    SchedulerUpdate { dispatchable: Vec<PItemId> },
    /// Touches-hook intercepted a worker writing outside its declared
    /// `touches`. Worker was SIGSTOPped; master decides next step.
    TouchesViolation {
        p_item_id: PItemId,
        attempted_path: String,
    },
    /// User added an Inbox material to a not-yet-running task. Carries the
    /// filename so the master can re-plan if needed.
    MaterialAdded { filename: String },
    /// Free-text event that doesn't fit the categories above.
    Custom(String),
}

/// User-originated message channel. Two flavours: prompted reply (came back
/// from AskUserQuestion) vs. unsolicited append (user typed text in the GUI).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserMessage {
    /// Reply to a master-initiated AskUserQuestion.
    AskReply { question_id: String, answer: String },
    /// Unprompted append (user typed a new instruction mid-task).
    Append(String),
}

/// Format an event for `append_user_message`. The first line is the
/// prefix-tagged summary; subsequent lines (if any) are details, indented
/// for readability. The master's SYSTEM prompt instructs it to split on
/// the `[event] ` / `[user] ` prefix when classifying.
pub fn format_event(event: &MasterEvent) -> String {
    match event {
        MasterEvent::WorkerCompleted { p_item_id, output_path } => {
            let suffix = match output_path {
                Some(p) => format!(", output at {p}"),
                None => String::new(),
            };
            format!("[event] worker for P-item {p_item_id} completed{suffix} — please run acceptance audit")
        }
        MasterEvent::WorkerFailed { p_item_id, reason } => {
            format!("[event] worker for P-item {p_item_id} FAILED: {reason:?} — decide retry / re-plan / escalate")
        }
        MasterEvent::SchedulerUpdate { dispatchable } => {
            if dispatchable.is_empty() {
                "[event] scheduler tick: no dispatchable P-items right now".to_string()
            } else {
                format!(
                    "[event] scheduler tick: now dispatchable = [{}]",
                    dispatchable.join(", ")
                )
            }
        }
        MasterEvent::TouchesViolation {
            p_item_id,
            attempted_path,
        } => format!(
            "[event] P-item {p_item_id} worker tried to modify undeclared file {attempted_path} — worker SIGSTOPped, decide补 touches / 打回 / 升级用户"
        ),
        MasterEvent::MaterialAdded { filename } => {
            format!("[event] user attached new material: {filename}")
        }
        MasterEvent::Custom(text) => format!("[event] {text}"),
    }
}

pub fn format_user(message: &UserMessage) -> String {
    match message {
        UserMessage::AskReply { question_id, answer } => {
            format!("[user] reply to question {question_id}: {answer}")
        }
        UserMessage::Append(text) => format!("[user] {text}"),
    }
}

/// Coalesces rapid-fire `SchedulerUpdate` events so the master isn't
/// flooded with N pings when one tick fires multiple dispatchable changes.
/// Other event variants pass through immediately.
///
/// Usage: call `submit` for each incoming event; `submit` returns `Some(msg)`
/// when the event should be appended now and `None` when it's been
/// queued for the debounce window. Call `flush_if_due` regularly (e.g.
/// once per 100 ms on a scheduler tick) to drain queued events whose
/// window has expired.
pub struct EventDebouncer {
    window: Duration,
    pending: Option<(Instant, MasterEvent)>,
}

impl EventDebouncer {
    /// 1-second window per PRD §5.7 / P21.
    pub fn new_default() -> Self {
        Self::new(Duration::from_millis(1000))
    }

    pub fn new(window: Duration) -> Self {
        Self {
            window,
            pending: None,
        }
    }

    /// Returns `Some(text)` when the event should be appended immediately;
    /// `None` when it's been queued (caller must drain via `flush_if_due`).
    pub fn submit(&mut self, event: MasterEvent) -> Option<String> {
        match &event {
            MasterEvent::SchedulerUpdate { .. } => {
                // Replace any pending scheduler update with the newest one —
                // master only cares about the latest dispatchable set.
                self.pending = Some((Instant::now(), event));
                None
            }
            _ => Some(format_event(&event)),
        }
    }

    /// Drain the pending event if its debounce window has elapsed.
    pub fn flush_if_due(&mut self) -> Option<String> {
        let (deadline_event, event) = match &self.pending {
            Some((t, e)) if t.elapsed() >= self.window => (t.clone(), e.clone()),
            _ => return None,
        };
        let _ = deadline_event;
        self.pending = None;
        Some(format_event(&event))
    }

    /// Force-flush any pending event regardless of the debounce window. Use
    /// when the supervisor is shutting down the master session.
    pub fn flush_all(&mut self) -> Option<String> {
        self.pending.take().map(|(_, e)| format_event(&e))
    }

    /// `true` if a scheduler update is queued.
    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

/// Diff helper for the supervisor's scheduler-update path: returns `true`
/// when the dispatchable set has materially changed (worth a notification).
pub fn dispatchable_changed(prev: &[PItemId], next: &[PItemId]) -> bool {
    let a: HashSet<&PItemId> = prev.iter().collect();
    let b: HashSet<&PItemId> = next.iter().collect();
    a != b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_event_uses_event_prefix() {
        let e = MasterEvent::WorkerCompleted {
            p_item_id: "p1".into(),
            output_path: Some("/tmp/p1.log".into()),
        };
        let s = format_event(&e);
        assert!(s.starts_with("[event]"));
        assert!(s.contains("p1"));
        assert!(s.contains("/tmp/p1.log"));
        assert!(s.contains("acceptance audit"));
    }

    #[test]
    fn format_event_worker_failed_carries_reason() {
        let e = MasterEvent::WorkerFailed {
            p_item_id: "p1".into(),
            reason: FailReason::BuildFailed,
        };
        let s = format_event(&e);
        assert!(s.starts_with("[event]"));
        assert!(s.contains("FAILED"));
        assert!(s.contains("BuildFailed"));
    }

    #[test]
    fn format_event_touches_violation_includes_attempted_path() {
        let e = MasterEvent::TouchesViolation {
            p_item_id: "p3".into(),
            attempted_path: "src/forbidden.rs".into(),
        };
        let s = format_event(&e);
        assert!(s.contains("p3"));
        assert!(s.contains("src/forbidden.rs"));
        assert!(s.contains("SIGSTOP"));
    }

    #[test]
    fn format_user_uses_user_prefix() {
        assert!(format_user(&UserMessage::Append("hi master".into())).starts_with("[user]"));
        assert!(format_user(&UserMessage::AskReply {
            question_id: "q1".into(),
            answer: "go ahead".into(),
        })
        .starts_with("[user]"));
    }

    #[test]
    fn debouncer_emits_non_scheduler_events_immediately() {
        let mut d = EventDebouncer::new(Duration::from_millis(50));
        let out = d.submit(MasterEvent::WorkerCompleted {
            p_item_id: "p1".into(),
            output_path: None,
        });
        assert!(out.is_some());
        assert!(!d.has_pending());
    }

    #[test]
    fn debouncer_coalesces_scheduler_updates() {
        let mut d = EventDebouncer::new(Duration::from_millis(50));
        assert!(d
            .submit(MasterEvent::SchedulerUpdate {
                dispatchable: vec!["p1".into()],
            })
            .is_none());
        // A second update inside the window replaces the first.
        assert!(d
            .submit(MasterEvent::SchedulerUpdate {
                dispatchable: vec!["p1".into(), "p2".into()],
            })
            .is_none());
        assert!(d.has_pending());

        // Before deadline → still none.
        assert!(d.flush_if_due().is_none());

        std::thread::sleep(Duration::from_millis(70));
        let flushed = d.flush_if_due().expect("should flush after deadline");
        // Should contain the newer set (p1+p2), not the older (just p1).
        assert!(flushed.contains("p1"));
        assert!(flushed.contains("p2"));
        assert!(!d.has_pending());
    }

    #[test]
    fn debouncer_flush_all_drains_immediately() {
        let mut d = EventDebouncer::new(Duration::from_secs(60));
        d.submit(MasterEvent::SchedulerUpdate {
            dispatchable: vec!["p1".into()],
        });
        assert!(d.has_pending());
        let flushed = d.flush_all().unwrap();
        assert!(flushed.contains("p1"));
        assert!(!d.has_pending());
    }

    #[test]
    fn dispatchable_changed_detects_set_diff() {
        let a = vec!["p1".to_string(), "p2".to_string()];
        let b = vec!["p2".to_string(), "p1".to_string()]; // same set, diff order
        let c = vec!["p1".to_string(), "p3".to_string()];
        assert!(!dispatchable_changed(&a, &b));
        assert!(dispatchable_changed(&a, &c));
    }

    #[test]
    fn material_added_event_includes_filename() {
        let e = MasterEvent::MaterialAdded {
            filename: "screenshot.png".into(),
        };
        let s = format_event(&e);
        assert!(s.contains("screenshot.png"));
    }
}
