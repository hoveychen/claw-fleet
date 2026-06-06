//! Plan-item (P-item) data model.
//!
//! A P-item is one node in a task's DAG plan. Defined per PRD §6.1
//! (design/task-as-unit-redesign.md). See `crate::plan::DagPlan` for the
//! container and `crate::dag` for the topology algorithms.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

pub type PItemId = String;

// [REQ-049] A resource name the scheduler treats as a mutual-exclusion lock:
// `build` / `test` / `git` / `custom:*` (PRD §6.3). Two P-items declaring the
// same ResourceName cannot run concurrently — see `DagPlan::resource_conflicts`.
/// `local:build`, `port:3000`, `global:simulator:ios`, etc. See PRD §6.3.
pub type ResourceName = String;

// [REQ-001] P-item is the canonical DAG-node record. The PRD's nine *atomic*
// fields — id, desc, depends_on, touches, resources, estimate_secs, acceptance,
// artifacts, skippable, human_gate, status — are all present below with derived
// Serialize/Deserialize. The runtime-bookkeeping tail (agent_session_id /
// started_at / completed_at / output_summary) is written by the supervisor, not
// the planner, so it stays `Option`; see the deviation note for why the spec's
// "no field is Option" wording does not extend to genuinely-absent runtime data.
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
    pub resources: Vec<ResourceName>,
    #[serde(default)]
    pub estimate_secs: Option<u64>,
    #[serde(default)]
    pub acceptance: Vec<AcceptanceCriterion>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactKind>,
    #[serde(default)]
    pub skippable: Option<SkipCondition>,
    // [REQ-011][REQ-050] When `true`, the master must enter `WaitHumanGate`
    // after the acceptance audit and ask the user before flipping to `Done` —
    // it never marks done unilaterally. Precedence with the task-level
    // `manual_review_all` flag is resolved by
    // `crate::acceptance::requires_human_gate` (either side gates).
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
    WaitResource,
    Running,
    // [REQ-011] Acceptance passed but a human gate (P-item `human_gate` or task
    // `manual_review_all`) is active — the audit yields `WaitHuman` and the
    // master parks the P-item here until the user approves, instead of `Done`.
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
    AcceptanceRejected,
    UpstreamFailed,
    UserRejected,
    Aborted,
    Custom(String),
}

// [REQ-012] The four verification methods. Master checks `PItem.acceptance` in
// declared order; each variant maps to one verification function (check_builds /
// check_tests / ask_human / eval_custom) wired up in P6's acceptance engine.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AcceptanceCriterion {
    Builds,
    TestsPass(String),
    HumanReview,
    Custom(String),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactKind {
    FileList,
    GitDiff,
    TestOutput,
    ManualNote,
}

// [REQ-048] Two skip conditions. `NoChangesIn(paths)` is auto-evaluated by the
// Master via `git diff` over those paths; `Custom(rule)` is evaluated by the
// worker, which returns a skip_result. Either way the Master records the reason
// via `DagPlan::propagate_skip` into the deviation ledger (REQ-007, P8).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SkipCondition {
    NoChangesIn(Vec<PathBuf>),
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

    /// `Running` or `WaitHumanGate` (worker has touched it, scheduler can't redispatch).
    pub fn is_active(&self) -> bool {
        matches!(self.status, PItemStatus::Running | PItemStatus::WaitHumanGate)
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
            resources: vec!["build".into()],
            estimate_secs: Some(120),
            acceptance: vec![
                AcceptanceCriterion::Builds,
                AcceptanceCriterion::TestsPass("cargo test".into()),
            ],
            artifacts: vec![ArtifactKind::FileList, ArtifactKind::GitDiff],
            skippable: Some(SkipCondition::NoChangesIn(vec![PathBuf::from("src/")])),
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
        // Older persisted P-items may not have all new patch fields. `serde(default)`
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

    #[test]
    fn terminal_and_active_predicates() {
        let mut p = sample();
        p.status = PItemStatus::WaitDeps;
        assert!(!p.is_terminal() && !p.is_active() && !p.unblocks_downstream());
        p.status = PItemStatus::Running;
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

    /// [REQ-001] Every atomic field the PRD enumerates must survive a JSON
    /// round-trip. We assert the serialized form actually carries each field key
    /// (so a field silently dropped from serde would fail here, not just an
    /// `assert_eq!` that could pass on two equally-broken values).
    #[test]
    fn all_atomic_fields_serialize_with_keys() {
        let p = sample();
        let v: serde_json::Value = serde_json::to_value(&p).unwrap();
        let obj = v.as_object().unwrap();
        for key in [
            "id",
            "desc",
            "touches",
            "dependsOn",
            "resources",
            "estimateSecs",
            "acceptance",
            "artifacts",
            "skippable",
            "humanGate",
            "status",
        ] {
            assert!(obj.contains_key(key), "missing serialized field key: {key}");
        }
        // And the round-trip is lossless.
        let back: PItem = serde_json::from_value(v).unwrap();
        assert_eq!(p, back);
    }

    /// [REQ-012] All four AcceptanceCriterion variants round-trip, including the
    /// String payloads of TestsPass / Custom. Master iterates `acceptance[*]` in
    /// declared order, so order preservation is asserted too.
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
        // Payload is genuinely carried, not dropped.
        match &back[1] {
            AcceptanceCriterion::TestsPass(cmd) => {
                assert_eq!(cmd, "cargo test -p claw-fleet-task")
            }
            other => panic!("expected TestsPass, got {other:?}"),
        }
    }

    /// [REQ-048] Both SkipCondition variants round-trip with their payloads.
    #[test]
    fn skip_condition_all_variants_roundtrip() {
        for sc in [
            SkipCondition::NoChangesIn(vec![PathBuf::from("src/"), PathBuf::from("Cargo.toml")]),
            SkipCondition::Custom("no migration needed".into()),
        ] {
            let json = serde_json::to_string(&sc).unwrap();
            let back: SkipCondition = serde_json::from_str(&json).unwrap();
            assert_eq!(sc, back);
        }
    }

    #[test]
    fn camel_case_status_serialization() {
        let p = PItem {
            id: "p1".into(),
            desc: "x".into(),
            touches: vec![],
            depends_on: vec![],
            resources: vec![],
            estimate_secs: None,
            acceptance: vec![],
            artifacts: vec![],
            skippable: None,
            human_gate: false,
            status: PItemStatus::WaitHumanGate,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        };
        let json = serde_json::to_string(&p).unwrap();
        assert!(json.contains("\"waitHumanGate\""));
    }
}
