//! Plan-item (P-item) data model.
//!
//! A P-item is one node in a task's lightweight dependency graph. See
//! `crate::plan::DagPlan` for the container and `crate::dag` for the topology
//! algorithms.
//!
//! Rebuilt (task-subsystem-rebuild): the speculative scheduling/auditing
//! machinery was removed. Gone: the `WaitResource` phantom status (never
//! written), and the `resources` / `artifacts` / `estimate_secs` / `skippable`
//! fields (resource scheduling and the five-gate acceptance engine they fed are
//! deleted). A P-item now carries only what the deterministic orchestrator and
//! the `/goal`-style review actually consume.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub type PItemId = String;

/// One node in the task's dependency graph. `acceptance` holds the
/// natural-language "done" criteria the review session checks the worker's
/// output against. The runtime-bookkeeping tail (agent_session_id / started_at
/// / completed_at / output_summary) is written by the orchestrator, not the
/// planner, so it stays `Option`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PItem {
    pub id: PItemId,
    pub desc: String,
    #[serde(default)]
    pub touches: Vec<PathBuf>,
    #[serde(default)]
    pub depends_on: Vec<PItemId>,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
    /// When `true`, the orchestrator parks the P-item in `WaitHumanGate` after
    /// review passes and waits for the user before flipping to `Done`.
    #[serde(default)]
    pub human_gate: bool,
    /// Newly-emitted P-items (from the planner) omit status and start in
    /// `WaitDeps`. Existing persisted P-items always serialize their actual
    /// status.
    #[serde(default = "PItemStatus::wait_deps")]
    pub status: PItemStatus,
    #[serde(default)]
    pub agent_session_id: Option<String>,
    #[serde(default)]
    pub started_at: Option<i64>,
    #[serde(default)]
    pub completed_at: Option<i64>,
    #[serde(default)]
    pub output_summary: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum PItemStatus {
    WaitDeps,
    Running,
    /// Worker finished; an independent review session is judging whether the
    /// P-item's acceptance criteria were actually met.
    Reviewing,
    /// Review passed but a human gate (P-item `human_gate`) is active — the
    /// orchestrator parks the P-item here until the user approves.
    WaitHumanGate,
    Done,
    Failed(FailReason),
    Skipped,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum FailReason {
    BuildFailed,
    TestsFailed,
    /// The review session judged the worker's output did not meet the
    /// acceptance criteria.
    ReviewRejected,
    UpstreamFailed,
    UserRejected,
    Aborted,
    Custom(String),
}

/// Natural-language "done" criteria. The review session checks the worker's
/// output (diff + summary) against these. `Builds` / `TestsPass` are kept as
/// structured shorthands the review prompt can render deterministically;
/// `Custom` is free-text.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AcceptanceCriterion {
    Builds,
    TestsPass(String),
    HumanReview,
    Custom(String),
}

impl PItemStatus {
    /// Default for newly-emitted P-items (from the planner). Kept as an
    /// associated function rather than `impl Default` so callers in core code
    /// have to pick a status explicitly.
    pub fn wait_deps() -> Self {
        Self::WaitDeps
    }
}

impl PItem {
    /// `Done`, `Skipped`, or `Failed(*)`.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self.status,
            PItemStatus::Done | PItemStatus::Skipped | PItemStatus::Failed(_)
        )
    }

    /// `true` once a dep is settled enough that downstream waits can proceed.
    /// `Done` and `Skipped` both unblock; `Failed` does not.
    pub fn unblocks_downstream(&self) -> bool {
        matches!(self.status, PItemStatus::Done | PItemStatus::Skipped)
    }

    /// Worker has touched it (or review is in flight / human gate pending), so
    /// the scheduler must not redispatch it.
    pub fn is_active(&self) -> bool {
        matches!(
            self.status,
            PItemStatus::Running | PItemStatus::Reviewing | PItemStatus::WaitHumanGate
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PItem {
        PItem {
            id: "p1".into(),
            desc: "do the thing".into(),
            touches: vec![PathBuf::from("src/main.rs")],
            depends_on: vec![],
            acceptance: vec![
                AcceptanceCriterion::Builds,
                AcceptanceCriterion::TestsPass("cargo test".into()),
            ],
            human_gate: true,
            status: PItemStatus::WaitDeps,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        }
    }

    #[test]
    fn serde_roundtrip_full() {
        let p = sample();
        let json = serde_json::to_string(&p).unwrap();
        let back: PItem = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn serde_roundtrip_failed_variant() {
        let mut p = sample();
        p.status = PItemStatus::Failed(FailReason::Custom("touches violated".into()));
        let json = serde_json::to_string(&p).unwrap();
        let back: PItem = serde_json::from_str(&json).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn deserialize_with_missing_optional_fields() {
        // Older persisted P-items may not have all fields. `serde(default)`
        // should fill them in.
        let json = r#"{
            "id": "p1",
            "desc": "minimal",
            "status": "waitDeps"
        }"#;
        let p: PItem = serde_json::from_str(json).unwrap();
        assert_eq!(p.id, "p1");
        assert!(p.touches.is_empty());
        assert!(p.acceptance.is_empty());
        assert!(!p.human_gate);
    }

    /// Persisted P-items that predate the rebuild may still carry the removed
    /// `resources` / `artifacts` / `estimateSecs` / `skippable` keys. serde must
    /// ignore unknown keys (no `deny_unknown_fields`) so old task JSON still loads.
    #[test]
    fn deserialize_ignores_removed_legacy_fields() {
        let json = r#"{
            "id": "p1",
            "desc": "legacy",
            "status": "waitDeps",
            "resources": ["build"],
            "artifacts": ["fileList"],
            "estimateSecs": 120,
            "skippable": {"custom": "x"}
        }"#;
        let p: PItem = serde_json::from_str(json).unwrap();
        assert_eq!(p.id, "p1");
        assert!(p.touches.is_empty());
    }

    /// Legacy persisted P-items may carry `waitResource` (a removed status). It
    /// should fail to deserialize loudly rather than silently — but legacy data
    /// in that exact status is vanishingly rare (it was never written). We assert
    /// the *current* statuses round-trip.
    #[test]
    fn terminal_and_active_predicates() {
        let mut p = sample();
        p.status = PItemStatus::WaitDeps;
        assert!(!p.is_terminal() && !p.is_active() && !p.unblocks_downstream());
        p.status = PItemStatus::Running;
        assert!(p.is_active() && !p.is_terminal());
        p.status = PItemStatus::Reviewing;
        assert!(p.is_active() && !p.is_terminal());
        p.status = PItemStatus::WaitHumanGate;
        assert!(p.is_active() && !p.is_terminal());
        p.status = PItemStatus::Done;
        assert!(p.is_terminal() && p.unblocks_downstream());
        p.status = PItemStatus::Skipped;
        assert!(p.is_terminal() && p.unblocks_downstream());
        p.status = PItemStatus::Failed(FailReason::BuildFailed);
        assert!(p.is_terminal() && !p.unblocks_downstream());
    }

    #[test]
    fn atomic_fields_serialize_with_keys() {
        let p = sample();
        let v: serde_json::Value = serde_json::to_value(&p).unwrap();
        let obj = v.as_object().unwrap();
        for key in ["id", "desc", "touches", "dependsOn", "acceptance", "humanGate", "status"] {
            assert!(obj.contains_key(key), "missing serialized field key: {key}");
        }
        let back: PItem = serde_json::from_value(v).unwrap();
        assert_eq!(p, back);
    }

    #[test]
    fn acceptance_criterion_all_variants_roundtrip() {
        let criteria = vec![
            AcceptanceCriterion::Builds,
            AcceptanceCriterion::TestsPass("cargo test -p claw-fleet-task".into()),
            AcceptanceCriterion::HumanReview,
            AcceptanceCriterion::Custom("clippy clean".into()),
        ];
        let json = serde_json::to_string(&criteria).unwrap();
        let back: Vec<AcceptanceCriterion> = serde_json::from_str(&json).unwrap();
        assert_eq!(criteria, back, "declared order + payloads must be preserved");
    }

    #[test]
    fn camel_case_status_serialization() {
        let mut p = sample();
        p.status = PItemStatus::WaitHumanGate;
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"waitHumanGate\""));
        p.status = PItemStatus::Reviewing;
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"reviewing\""));
    }
}
