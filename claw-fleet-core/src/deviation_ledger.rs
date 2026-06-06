//! Deviation ledger + P-item state-transition audit log — methodology pillar 3
//! (deviation ledger) and pillar 2 (traceability matrix) physical carriers.
//!
//! Two append-only JSONL logs live here, both under `~/.fleet/`:
//!
//! - **`deviations.jsonl`** ([REQ-025]) — one [`DeviationEntry`] per line. Every
//!   decision that does NOT fully satisfy the spec (a simplification, a deferred
//!   REQ, a touches violation accepted by the master) is appended here. The log
//!   is append-only: [`append_deviation`] never rewrites or truncates the file,
//!   so the full history of who-approved-what is auditable. A reader
//!   ([`read_deviations`]) returns entries in the order they were written (file
//!   order == chronological append order).
//!
//! - **`task-audit.jsonl`** ([REQ-008] [REQ-039]) — one [`StateTransition`] per
//!   line. Every runtime P-item status transition (`WaitDeps → Running`,
//!   `Running → Done|Failed|Skipped`, etc.) is appended here with its timestamp
//!   and the thing that triggered it ([`TransitionTrigger`] — a REQ id, a worker
//!   session, or a user action). Users can replay a P-item's full status history
//!   from this log.
//!
//! ## Append-only discipline
//!
//! Both writers use `OpenOptions::append(true).create(true)` and write exactly
//! one `\n`-terminated JSON object per call — no read-modify-write, so concurrent
//! appends from different Fleet processes interleave at line granularity without
//! clobbering each other (POSIX guarantees small `O_APPEND` writes are atomic).
//! Neither writer ever opens the file for truncation, and there is no public
//! "rewrite" or "delete entry" API by design: the ledger is the audit trail, and
//! tampering with it would defeat methodology pillar 3.
//!
//! This module is pure model + file I/O; the Master's `mark-done` enforcement
//! (REQ-027: refuse Done while a relevant deviation is `Pending`) lives in P9's
//! `actions.rs` and consumes [`read_deviations`].

use std::io::Write;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::pitem::PItemId;
use crate::session::get_fleet_dir;

/// Approval state of a deviation. The Master refuses to mark a P-item `Done`
/// while a deviation touching one of its REQs is still [`DeviationStatus::Pending`]
/// (enforced in P9 via [`read_deviations`]).
// [REQ-025]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum DeviationStatus {
    /// Recorded but not yet signed off by a human. Blocks dependent `mark-done`.
    Pending,
    /// A human (`approved_by`) signed off at `signed_off_at`.
    Approved,
    /// Reviewed and rejected — the deviation must be remediated, not accepted.
    Rejected,
}

/// Severity of the spec deviation, mirroring the audit-pattern risk tiers.
// [REQ-025]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum RiskLevel {
    Low,
    Medium,
    High,
    Critical,
}

/// One append-only deviation-ledger entry. Carries every field the PRD's
/// methodology pillar 3 requires: `id, date, affected_req, decision, rationale,
/// risk_level, mitigation, approved_by, signed_off_at, status`.
///
/// `approved_by` / `signed_off_at` are `Option` because a freshly-appended
/// `Pending` entry has not been signed off yet — they are filled in (by
/// appending a superseding entry; the log is append-only) when a human approves.
// [REQ-025]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DeviationEntry {
    /// Stable id, e.g. `DEV-001`. Uniqueness is the caller's responsibility;
    /// the ledger preserves duplicates faithfully (append-only — it does not
    /// dedup or overwrite).
    pub id: String,
    /// ISO-8601 date the deviation was recorded (e.g. `2026-06-06`).
    pub date: String,
    /// REQ ids this deviation weakens / defers, e.g. `["REQ-007", "REQ-008"]`.
    pub affected_req: Vec<String>,
    /// The decision taken (what was simplified / deferred / accepted).
    pub decision: String,
    /// Why the decision was acceptable.
    pub rationale: String,
    pub risk_level: RiskLevel,
    /// How the residual risk is mitigated (test added, follow-up filed, etc.).
    pub mitigation: String,
    /// Human who approved. `None` while `status == Pending`.
    pub approved_by: Option<String>,
    /// Unix epoch seconds the human signed off. `None` while `status == Pending`.
    pub signed_off_at: Option<i64>,
    pub status: DeviationStatus,
}

/// `~/.fleet/deviations.jsonl`. [REQ-025]
pub fn deviations_path() -> Result<PathBuf, String> {
    get_fleet_dir()
        .map(|d| d.join("deviations.jsonl"))
        .ok_or_else(|| "could not resolve fleet home dir".to_string())
}

/// Append one deviation entry as a single JSON line. Append-only: this never
/// truncates or rewrites the file, so earlier entries are preserved verbatim.
/// Returns the path written. [REQ-025]
pub fn append_deviation(entry: &DeviationEntry) -> Result<PathBuf, String> {
    let path = deviations_path()?;
    append_jsonl_line(&path, entry)?;
    Ok(path)
}

/// Read every deviation entry in append (chronological) order. Malformed lines
/// are skipped rather than failing the whole read, so one corrupt append can't
/// hide the rest of the audit trail. [REQ-025]
pub fn read_deviations() -> Result<Vec<DeviationEntry>, String> {
    let path = deviations_path()?;
    read_jsonl_lines(&path)
}

// ── State-transition audit log ───────────────────────────────────────────────

/// What triggered a P-item state transition. [REQ-008] requires the source REQ
/// id be recorded; [REQ-039] generalises the trigger to also cover the worker
/// session that drove it or an explicit user action.
// [REQ-008] [REQ-039]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TransitionTrigger {
    /// The transition is attributed to a registered requirement (e.g. the
    /// `propagate_skip` rule, which is REQ-007). [REQ-008]
    ReqId(String),
    /// A worker subprocess (Claude Code session UUID) drove the transition,
    /// e.g. `WaitDeps → Running` on dispatch.
    SessionId(String),
    /// A human action (e.g. user rejected at a human gate).
    UserAction(String),
}

/// One append-only P-item status transition record. [REQ-008] [REQ-039]
// [REQ-008] [REQ-039]
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StateTransition {
    pub p_item_id: PItemId,
    /// Serialized status names (`waitDeps`, `running`, `done`, …) — kept as
    /// strings so the audit log does not have to depend on the `PItemStatus`
    /// enum shape and stays readable when replayed.
    pub from_status: String,
    pub to_status: String,
    /// Unix epoch seconds the transition happened.
    pub timestamp: i64,
    pub trigger: TransitionTrigger,
}

/// `~/.fleet/task-audit.jsonl`. [REQ-039]
pub fn audit_log_path() -> Result<PathBuf, String> {
    get_fleet_dir()
        .map(|d| d.join("task-audit.jsonl"))
        .ok_or_else(|| "could not resolve fleet home dir".to_string())
}

/// Append one state-transition record as a single JSON line. Append-only.
/// Returns the path written. [REQ-008] [REQ-039]
pub fn append_transition(t: &StateTransition) -> Result<PathBuf, String> {
    let path = audit_log_path()?;
    append_jsonl_line(&path, t)?;
    Ok(path)
}

/// Append a batch of transitions in order (e.g. the skip records returned by
/// `DagPlan::propagate_skip`). Each is one JSONL line. [REQ-008] [REQ-039]
pub fn append_transitions(ts: &[StateTransition]) -> Result<(), String> {
    let path = audit_log_path()?;
    for t in ts {
        append_jsonl_line(&path, t)?;
    }
    Ok(())
}

/// Read every state transition in append (chronological) order. [REQ-039]
pub fn read_transitions() -> Result<Vec<StateTransition>, String> {
    let path = audit_log_path()?;
    read_jsonl_lines(&path)
}

/// Read every state transition for one P-item, in append order — the user-facing
/// "show this P-item's status history" query of [REQ-039].
pub fn transitions_for(p_item_id: &str) -> Result<Vec<StateTransition>, String> {
    Ok(read_transitions()?
        .into_iter()
        .filter(|t| t.p_item_id == p_item_id)
        .collect())
}

// ── Shared append-only JSONL primitives ──────────────────────────────────────

/// Append one `\n`-terminated JSON object to `path`, creating it (and its parent
/// dir) if absent. Uses `O_APPEND` so the write is atomic at line granularity
/// across processes; never truncates. The serialized value must NOT contain a
/// newline (serde_json's compact form never emits one), so each line is exactly
/// one record.
fn append_jsonl_line<T: Serialize>(path: &PathBuf, value: &T) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir ledger dir: {e}"))?;
    }
    let mut line = serde_json::to_string(value).map_err(|e| format!("serialize ledger entry: {e}"))?;
    line.push('\n');
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| format!("open ledger {}: {e}", path.display()))?;
    f.write_all(line.as_bytes())
        .map_err(|e| format!("append ledger line: {e}"))?;
    Ok(())
}

/// Read & parse every line of a JSONL file in file (== append) order. A missing
/// file is an empty result; malformed lines are skipped (best-effort) so one bad
/// append can't blind the whole audit trail.
fn read_jsonl_lines<T: for<'de> Deserialize<'de>>(path: &PathBuf) -> Result<Vec<T>, String> {
    if !path.exists() {
        return Ok(vec![]);
    }
    let content = std::fs::read_to_string(path).map_err(|e| format!("read ledger: {e}"))?;
    let mut out = Vec::new();
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<T>(line) {
            out.push(v);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    /// Mirror of touches_hook's `FleetHomeOverride`: point `FLEET_HOME` at a
    /// temp dir so the ledgers write under an isolated `~/.fleet`.
    struct FleetHomeOverride {
        prev: Option<std::ffi::OsString>,
    }

    impl FleetHomeOverride {
        fn new(tmp: &Path) -> Self {
            let prev = std::env::var_os("FLEET_HOME");
            unsafe { std::env::set_var("FLEET_HOME", tmp) };
            FleetHomeOverride { prev }
        }
    }

    impl Drop for FleetHomeOverride {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(p) => std::env::set_var("FLEET_HOME", p),
                    None => std::env::remove_var("FLEET_HOME"),
                }
            }
        }
    }

    fn dev(id: &str, status: DeviationStatus) -> DeviationEntry {
        DeviationEntry {
            id: id.into(),
            date: "2026-06-06".into(),
            affected_req: vec!["REQ-007".into()],
            decision: "simplified X".into(),
            rationale: "Y was out of scope".into(),
            risk_level: RiskLevel::Medium,
            mitigation: "added follow-up test".into(),
            approved_by: match status {
                DeviationStatus::Approved => Some("boss".into()),
                _ => None,
            },
            signed_off_at: match status {
                DeviationStatus::Approved => Some(1_700_000_000),
                _ => None,
            },
            status,
        }
    }

    /// [REQ-025] Append-only: write several entries, read them back in order,
    /// none lost, none overwritten. Then append more and confirm the earlier
    /// ones are still present and still first (the file was extended, not
    /// rewritten).
    #[test]
    fn req025_deviations_append_only_preserves_order_and_no_loss() {
        let _g = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());

        append_deviation(&dev("DEV-001", DeviationStatus::Pending)).unwrap();
        append_deviation(&dev("DEV-002", DeviationStatus::Approved)).unwrap();
        append_deviation(&dev("DEV-003", DeviationStatus::Rejected)).unwrap();

        let back = read_deviations().unwrap();
        assert_eq!(back.len(), 3, "all three entries must be readable");
        assert_eq!(
            back.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            vec!["DEV-001", "DEV-002", "DEV-003"],
            "entries must come back in append order"
        );
        assert_eq!(back[0].status, DeviationStatus::Pending);
        assert_eq!(back[1].status, DeviationStatus::Approved);
        assert_eq!(back[1].approved_by.as_deref(), Some("boss"));

        // Append two more; the earlier three must be untouched and still first.
        append_deviation(&dev("DEV-004", DeviationStatus::Pending)).unwrap();
        append_deviation(&dev("DEV-005", DeviationStatus::Pending)).unwrap();
        let back2 = read_deviations().unwrap();
        assert_eq!(back2.len(), 5, "append extended the log, nothing lost");
        assert_eq!(
            back2.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(),
            vec!["DEV-001", "DEV-002", "DEV-003", "DEV-004", "DEV-005"]
        );
        // Byte-level proof of no-overwrite: the raw file has exactly 5 lines.
        let raw = std::fs::read_to_string(deviations_path().unwrap()).unwrap();
        assert_eq!(raw.lines().filter(|l| !l.trim().is_empty()).count(), 5);
    }

    /// [REQ-025] Full round-trip of every field, including the optional
    /// `approved_by` / `signed_off_at` on an Approved entry.
    #[test]
    fn req025_deviation_entry_roundtrips_all_fields() {
        let _g = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());

        let e = DeviationEntry {
            id: "DEV-042".into(),
            date: "2026-06-06".into(),
            affected_req: vec!["REQ-025".into(), "REQ-027".into()],
            decision: "deferred enforcement".into(),
            rationale: "P9 owns the gate".into(),
            risk_level: RiskLevel::High,
            mitigation: "documented in registry notes".into(),
            approved_by: Some("老板".into()),
            signed_off_at: Some(1_717_000_000),
            status: DeviationStatus::Approved,
        };
        append_deviation(&e).unwrap();
        let back = read_deviations().unwrap();
        assert_eq!(back.len(), 1);
        assert_eq!(back[0], e, "every field must survive the JSONL round-trip");
    }

    /// [REQ-025] Reading a ledger that was never created yields an empty vec,
    /// not an error.
    #[test]
    fn req025_read_missing_deviations_is_empty() {
        let _g = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());
        assert!(read_deviations().unwrap().is_empty());
    }

    /// [REQ-008] [REQ-039] State transitions append-only, queryable per P-item,
    /// carrying (from, to, ts, trigger). Two P-items interleaved; per-item
    /// query returns only that item's chain, in append order.
    #[test]
    fn req008_req039_transition_audit_append_and_query() {
        let _g = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());

        append_transition(&StateTransition {
            p_item_id: "p1".into(),
            from_status: "waitDeps".into(),
            to_status: "running".into(),
            timestamp: 100,
            trigger: TransitionTrigger::SessionId("sess-abc".into()),
        })
        .unwrap();
        append_transition(&StateTransition {
            p_item_id: "p2".into(),
            from_status: "waitDeps".into(),
            to_status: "skipped".into(),
            timestamp: 101,
            trigger: TransitionTrigger::ReqId("REQ-007".into()),
        })
        .unwrap();
        append_transition(&StateTransition {
            p_item_id: "p1".into(),
            from_status: "running".into(),
            to_status: "done".into(),
            timestamp: 200,
            trigger: TransitionTrigger::SessionId("sess-abc".into()),
        })
        .unwrap();

        let all = read_transitions().unwrap();
        assert_eq!(all.len(), 3, "all transitions readable in append order");
        assert_eq!(all[0].timestamp, 100);
        assert_eq!(all[2].to_status, "done");

        // [REQ-039] user-facing per-P-item history query.
        let p1 = transitions_for("p1").unwrap();
        assert_eq!(p1.len(), 2, "p1 has WaitDeps→Running→Done");
        assert_eq!(p1[0].from_status, "waitDeps");
        assert_eq!(p1[0].to_status, "running");
        assert_eq!(p1[1].from_status, "running");
        assert_eq!(p1[1].to_status, "done");

        // [REQ-008] the skip transition records the REQ source that triggered it.
        let p2 = transitions_for("p2").unwrap();
        assert_eq!(p2.len(), 1);
        assert_eq!(p2[0].trigger, TransitionTrigger::ReqId("REQ-007".into()));
        assert_eq!(p2[0].to_status, "skipped");
    }

    /// [REQ-008] [REQ-039] `append_transitions` batch helper writes each as its
    /// own line and preserves order.
    #[test]
    fn req039_batch_append_transitions() {
        let _g = crate::session::fleet_home_lock();
        let tmp = tempfile::TempDir::new().unwrap();
        let _o = FleetHomeOverride::new(tmp.path());

        let batch = vec![
            StateTransition {
                p_item_id: "b".into(),
                from_status: "waitDeps".into(),
                to_status: "skipped".into(),
                timestamp: 10,
                trigger: TransitionTrigger::ReqId("REQ-007".into()),
            },
            StateTransition {
                p_item_id: "c".into(),
                from_status: "waitDeps".into(),
                to_status: "skipped".into(),
                timestamp: 11,
                trigger: TransitionTrigger::ReqId("REQ-007".into()),
            },
        ];
        append_transitions(&batch).unwrap();
        let back = read_transitions().unwrap();
        assert_eq!(back.len(), 2);
        assert_eq!(back[0].p_item_id, "b");
        assert_eq!(back[1].p_item_id, "c");
    }
}
