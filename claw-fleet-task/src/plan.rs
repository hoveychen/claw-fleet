//! `DagPlan` — task-level container for a set of `PItem` nodes plus the
//! plan-level operations the orchestrator needs (topology, blocked-set
//! computation, skip propagation).
//!
//! Pure DAG primitives live in `crate::dag`; this module specialises them for
//! `PItem` state.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::dag;
use crate::pitem::{PItem, PItemId, PItemStatus};

// Task-level container: an immutable-by-convention map of P-item id →
// P-item plus the accessors the scheduler needs. `node_ids()` gives a
// deterministic iteration order; `validate()` is the pre-run integrity gate
// (reference completeness + acyclicity).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DagPlan {
    pub items: HashMap<PItemId, PItem>,
}

/// One auto-skip event produced by [`DagPlan::propagate_skip_with_evidence`].
///
/// requires every skipped P-item to carry the decision evidence the
/// deviation ledger needs to trace *why* a node was skipped: the id, a
/// human-readable reason, the wall-clock timestamp, and the session that drove
/// the skip (the master/scheduler session that ran propagation). The deviation
/// ledger consumer (P8 `deviation_ledger`, P9 `actions`) turns each record into
/// a ledger / state-transition entry so users can query which P-items were
/// auto-skipped because of an upstream failure and when.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkipRecord {
    /// The P-item that was flipped to `Skipped`.
    pub p_item_id: PItemId,
    /// Why it was skipped — names the failed upstream ancestor that poisoned it.
    pub skip_reason: String,
    /// Unix epoch seconds the skip was decided.
    pub skipped_at_timestamp: i64,
    /// The session that ran propagation (master/scheduler), if known. `None`
    /// when propagation runs outside a session context (e.g. a unit test).
    pub skipped_by_session_id: Option<String>,
}

/// Reference-integrity issue surfaced by `validate`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanValidationError {
    /// `referenced_by.depends_on` mentions `missing`, which is not in the plan.
    MissingDep {
        referenced_by: PItemId,
        missing: PItemId,
    },
    /// The graph contains a cycle. `path` walks the witness with the closing
    /// node appended (so `path.first() == path.last()`).
    Cycle { path: Vec<PItemId> },
}

impl DagPlan {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn from_items(items: impl IntoIterator<Item = PItem>) -> Self {
        let items = items.into_iter().map(|p| (p.id.clone(), p)).collect();
        Self { items }
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
    pub fn get(&self, id: &str) -> Option<&PItem> {
        self.items.get(id)
    }
    pub fn get_mut(&mut self, id: &str) -> Option<&mut PItem> {
        self.items.get_mut(id)
    }

    /// Stable list of node ids — sorted lexicographically so iteration order is
    /// deterministic regardless of `HashMap` insertion order, and tests/schedulers
    /// can compare against a fixed order.
    // Deterministic node iteration: the scheduler and traceability
    // layer both depend on a stable ordering that does not vary across runs.
    pub fn node_ids(&self) -> Vec<PItemId> {
        let mut ids: Vec<PItemId> = self.items.keys().cloned().collect();
        ids.sort();
        ids
    }

    fn deps_of(&self, id: &PItemId) -> Vec<PItemId> {
        self.items
            .get(id)
            .map(|p| p.depends_on.clone())
            .unwrap_or_default()
    }

    /// Topological order over all P-items; returns the cycle witness on failure.
    pub fn topo_sort(&self) -> Result<Vec<PItemId>, Vec<PItemId>> {
        let ids = self.node_ids();
        dag::topo_sort(&ids, |id| self.deps_of(id))
    }

    /// Cycle witness if any. See `dag::find_cycle` for path shape.
    pub fn cycle_detect(&self) -> Option<Vec<PItemId>> {
        let ids = self.node_ids();
        dag::find_cycle(&ids, |id| self.deps_of(id))
    }

    /// Reference + acyclicity check. Returns the first issue encountered.
    // Pre-run integrity gate: every `depends_on` must resolve to a
    // node in the plan, and the graph must be acyclic, before the scheduler
    // dispatches anything.
    pub fn validate(&self) -> Result<(), PlanValidationError> {
        for (id, p) in &self.items {
            for d in &p.depends_on {
                if !self.items.contains_key(d) {
                    return Err(PlanValidationError::MissingDep {
                        referenced_by: id.clone(),
                        missing: d.clone(),
                    });
                }
            }
        }
        if let Some(path) = self.cycle_detect() {
            return Err(PlanValidationError::Cycle { path });
        }
        Ok(())
    }

    /// IDs of P-items currently `WaitDeps` whose every dependency is settled
    /// (`Done` or `Skipped`). Scheduler uses this as the candidate set before
    /// applying resource-lock filtering.
    // Returns the `WaitDeps` nodes whose `depends_on` are all terminal
    // (Done/Skipped — Failed does not unblock; see `unblocks_downstream`). The
    // result is sorted for determinism so the scheduler picks reproducibly.
    pub fn unblocked_items(&self) -> Vec<PItemId> {
        let mut out: Vec<PItemId> = self
            .items
            .iter()
            .filter(|(_, p)| matches!(p.status, PItemStatus::WaitDeps))
            .filter(|(_, p)| {
                p.depends_on.iter().all(|d| {
                    self.items
                        .get(d)
                        .map(|x| x.unblocks_downstream())
                        .unwrap_or(false)
                })
            })
            .map(|(id, _)| id.clone())
            .collect();
        out.sort();
        out
    }

    /// Transitively flip every `WaitDeps` descendant of a `Failed` ancestor
    /// to `Skipped`. Already-terminal descendants are left alone. Returns the
    /// sorted set of newly-skipped IDs.
    ///
    /// Convention: `Failed` poisons; `Skipped` and `Done` both unblock and do
    /// not poison. So a `Skipped` parent leaves children to run normally.
    ///
    /// This is the bare-id form kept for callers that only need the set of
    /// skipped ids (scheduler, `actions::mark_failed`). When the deviation
    /// ledger needs the *evidence* behind each skip — reason, timestamp,
    /// triggering session — use [`propagate_skip_with_evidence`] instead
    /// (); this method delegates to it and discards the evidence.
    pub fn propagate_skip(&mut self) -> Vec<PItemId> {
        let mut ids: Vec<PItemId> = self
            .propagate_skip_with_evidence(0, None)
            .into_iter()
            .map(|r| r.p_item_id)
            .collect();
        ids.sort();
        ids
    }

    /// Same transitive skip propagation as [`propagate_skip`], but returns a
    /// [`SkipRecord`] for each newly-skipped P-item carrying the decision
    /// evidence the deviation ledger tracks: the skip reason (which failed
    /// upstream ancestor poisoned it), the timestamp, and the session that ran
    /// propagation. Records are returned in sorted-by-id order for determinism.
    ///
    /// `now` is the Unix-epoch-seconds timestamp to stamp on each skip (the
    /// caller passes `chrono::Utc::now().timestamp()`; tests pass a fixed
    /// value). `session_id` is the master/scheduler session driving the skip,
    /// if any.
    ///
    /// The returned records are the evidence the orchestrator turns into
    /// `WaitDeps → Skipped` entries in the task's audit log so each auto-skip's
    /// (from, to, ts, trigger) is persisted.
    pub fn propagate_skip_with_evidence(
        &mut self,
        now: i64,
        session_id: Option<String>,
    ) -> Vec<SkipRecord> {
        // Forward adjacency: dep → [dependants]
        let mut dependants: HashMap<PItemId, Vec<PItemId>> = HashMap::new();
        for (id, p) in &self.items {
            for d in &p.depends_on {
                dependants.entry(d.clone()).or_default().push(id.clone());
            }
        }

        // Seed the frontier with the failed roots. We carry, alongside each
        // frontier node, the originating failed ancestor so a skipped child can
        // name *why* it was skipped (which upstream failure poisoned its chain).
        let mut newly_skipped: Vec<SkipRecord> = Vec::new();
        let mut frontier: Vec<(PItemId, PItemId)> = self
            .items
            .iter()
            .filter(|(_, p)| matches!(p.status, PItemStatus::Failed(_)))
            .map(|(id, _)| (id.clone(), id.clone()))
            .collect();

        let mut visited: HashSet<PItemId> = HashSet::new();
        while let Some((node, failed_root)) = frontier.pop() {
            if !visited.insert(node.clone()) {
                continue;
            }
            if let Some(children) = dependants.get(&node) {
                for c in children.clone() {
                    let Some(child) = self.items.get_mut(&c) else {
                        continue;
                    };
                    if matches!(child.status, PItemStatus::WaitDeps) {
                        child.status = PItemStatus::Skipped;
                        newly_skipped.push(SkipRecord {
                            p_item_id: c.clone(),
                            skip_reason: format!(
                                "auto-skipped: upstream P-item '{failed_root}' failed"
                            ),
                            skipped_at_timestamp: now,
                            skipped_by_session_id: session_id.clone(),
                        });
                        // The originating failed root propagates further down so
                        // a grand-child blames the same root failure.
                        frontier.push((c, failed_root.clone()));
                    }
                }
            }
        }
        newly_skipped.sort_by(|a, b| a.p_item_id.cmp(&b.p_item_id));
        newly_skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitem::{FailReason, PItemStatus};

    fn item(id: &str, deps: &[&str], status: PItemStatus) -> PItem {
        PItem {
            id: id.into(),
            desc: id.into(),
            touches: vec![],
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            acceptance: vec![],
            human_gate: false,
            status,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
            failure_gaps: Vec::new(),
        }
    }

    #[test]
    fn topo_sort_orders_deps_before_dependants() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::WaitDeps),
            item("b", &["a"], PItemStatus::WaitDeps),
            item("c", &["b"], PItemStatus::WaitDeps),
        ]);
        let order = plan.topo_sort().unwrap();
        let pos = |x: &str| order.iter().position(|s| s == x).unwrap();
        assert!(pos("a") < pos("b") && pos("b") < pos("c"));
    }

    #[test]
    fn cycle_detect_returns_witness() {
        let plan = DagPlan::from_items(vec![
            item("a", &["b"], PItemStatus::WaitDeps),
            item("b", &["a"], PItemStatus::WaitDeps),
        ]);
        let cycle = plan.cycle_detect().expect("cycle expected");
        assert_eq!(cycle.first(), cycle.last());
        assert!(plan.topo_sort().is_err());
    }

    #[test]
    fn validate_missing_dep() {
        let plan = DagPlan::from_items(vec![item("a", &["ghost"], PItemStatus::WaitDeps)]);
        match plan.validate() {
            Err(PlanValidationError::MissingDep { referenced_by, missing }) => {
                assert_eq!(referenced_by, "a");
                assert_eq!(missing, "ghost");
            }
            other => panic!("expected MissingDep, got {other:?}"),
        }
    }

    #[test]
    fn validate_cycle() {
        let plan = DagPlan::from_items(vec![
            item("a", &["b"], PItemStatus::WaitDeps),
            item("b", &["a"], PItemStatus::WaitDeps),
        ]);
        assert!(matches!(
            plan.validate(),
            Err(PlanValidationError::Cycle { .. })
        ));
    }

    /// `node_ids()` must be deterministic regardless of the order
    /// items were inserted into the underlying HashMap. Build the same plan two
    /// ways (forward + reversed insertion) and assert identical ordering.
    #[test]
    fn node_ids_is_deterministic_across_insertion_order() {
        let forward = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::WaitDeps),
            item("b", &[], PItemStatus::WaitDeps),
            item("c", &[], PItemStatus::WaitDeps),
        ]);
        let reversed = DagPlan::from_items(vec![
            item("c", &[], PItemStatus::WaitDeps),
            item("b", &[], PItemStatus::WaitDeps),
            item("a", &[], PItemStatus::WaitDeps),
        ]);
        assert_eq!(forward.node_ids(), vec!["a", "b", "c"]);
        assert_eq!(forward.node_ids(), reversed.node_ids());
    }

    /// Basic accessors exist and behave: get / get_mut / len.
    #[test]
    fn basic_accessors() {
        let mut plan = DagPlan::from_items(vec![item("a", &[], PItemStatus::WaitDeps)]);
        assert_eq!(plan.len(), 1);
        assert!(plan.get("a").is_some());
        assert!(plan.get("missing").is_none());
        plan.get_mut("a").unwrap().desc = "edited".into();
        assert_eq!(plan.get("a").unwrap().desc, "edited");
    }

    /// Registry scenario: nodes 2 and 3 both depend on 1. While 1 is
    /// WaitDeps nothing is unblocked; once 1 is Done both 2 and 3 unblock, in
    /// sorted order.
    #[test]
    fn unblocked_items_registry_scenario_1_unblocks_2_and_3() {
        let mut plan = DagPlan::from_items(vec![
            item("1", &[], PItemStatus::WaitDeps),
            item("2", &["1"], PItemStatus::WaitDeps),
            item("3", &["1"], PItemStatus::WaitDeps),
        ]);
        // Node 1 has no deps, so it is the only thing unblocked initially; 2 and
        // 3 still wait on it.
        assert_eq!(plan.unblocked_items(), vec!["1".to_string()]);
        plan.get_mut("1").unwrap().status = PItemStatus::Done;
        // Now 1 is terminal-and-unblocking; 2 and 3 become runnable in sorted order.
        assert_eq!(plan.unblocked_items(), vec!["2".to_string(), "3".to_string()]);
    }

    #[test]
    fn unblocked_items_requires_all_deps_settled() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Done),
            item("b", &["a"], PItemStatus::WaitDeps),
            item("c", &["a", "b"], PItemStatus::WaitDeps),
        ]);
        // Only "b" is unblocked — "c" still waits on "b".
        assert_eq!(plan.unblocked_items(), vec!["b".to_string()]);
    }

    #[test]
    fn unblocked_items_ignores_already_running() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Done),
            item("b", &["a"], PItemStatus::Running),
        ]);
        assert!(plan.unblocked_items().is_empty());
    }

    #[test]
    fn unblocked_items_treats_skipped_as_settled() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Skipped),
            item("b", &["a"], PItemStatus::WaitDeps),
        ]);
        assert_eq!(plan.unblocked_items(), vec!["b".to_string()]);
    }

    #[test]
    fn propagate_skip_marks_transitive_descendants() {
        let mut plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Failed(FailReason::BuildFailed)),
            item("b", &["a"], PItemStatus::WaitDeps),
            item("c", &["b"], PItemStatus::WaitDeps),
            // Unrelated chain — should stay WaitDeps.
            item("x", &[], PItemStatus::Done),
            item("y", &["x"], PItemStatus::WaitDeps),
        ]);
        let skipped = plan.propagate_skip();
        assert_eq!(skipped, vec!["b".to_string(), "c".to_string()]);
        assert!(matches!(plan.get("b").unwrap().status, PItemStatus::Skipped));
        assert!(matches!(plan.get("c").unwrap().status, PItemStatus::Skipped));
        assert!(matches!(plan.get("y").unwrap().status, PItemStatus::WaitDeps));
    }

    #[test]
    fn propagate_skip_does_not_overwrite_terminal() {
        let mut plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Failed(FailReason::BuildFailed)),
            // Already Done — don't touch.
            item("b", &["a"], PItemStatus::Done),
        ]);
        let skipped = plan.propagate_skip();
        assert!(skipped.is_empty());
        assert!(matches!(plan.get("b").unwrap().status, PItemStatus::Done));
    }

    #[test]
    fn propagate_skip_does_not_propagate_through_skipped_parent() {
        // PRD §6.1 convention: only Failed poisons. Skipped should leave kids alone
        // (they may still be runnable through other deps).
        let mut plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Skipped),
            item("b", &["a"], PItemStatus::WaitDeps),
        ]);
        let skipped = plan.propagate_skip();
        assert!(skipped.is_empty());
        assert!(matches!(plan.get("b").unwrap().status, PItemStatus::WaitDeps));
    }

    /// propagate_skip_with_evidence: each skipped P-item carries
    /// (p_item_id, skip_reason naming the failed upstream, timestamp, session).
    /// Registry scenario: upstream Failed → downstream Skipped, and the evidence
    /// is sufficient for the deviation ledger to trace the cause.
    #[test]
    fn req007_propagate_skip_with_evidence_carries_reason_ts_session() {
        let mut plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Failed(FailReason::BuildFailed)),
            item("b", &["a"], PItemStatus::WaitDeps),
            item("c", &["b"], PItemStatus::WaitDeps),
        ]);
        let records =
            plan.propagate_skip_with_evidence(1_717_000_000, Some("sess-master".into()));

        assert_eq!(records.len(), 2, "b and c both skipped");
        // Sorted by id for determinism.
        assert_eq!(records[0].p_item_id, "b");
        assert_eq!(records[1].p_item_id, "c");

        for r in &records {
            // Timestamp + session are propagated onto every record.
            assert_eq!(r.skipped_at_timestamp, 1_717_000_000);
            assert_eq!(r.skipped_by_session_id.as_deref(), Some("sess-master"));
            // Reason names the failed upstream root 'a' — enough to trace cause.
            assert!(
                r.skip_reason.contains("'a'") && r.skip_reason.contains("failed"),
                "reason must name the failed upstream ancestor: {}",
                r.skip_reason
            );
        }
        // State actually flipped.
        assert!(matches!(plan.get("b").unwrap().status, PItemStatus::Skipped));
        assert!(matches!(plan.get("c").unwrap().status, PItemStatus::Skipped));
    }

    /// The bare `propagate_skip` wrapper still returns the same sorted
    /// id set (the evidence form must not regress P2's existing contract).
    #[test]
    fn req007_bare_propagate_skip_matches_evidence_ids() {
        let make = || {
            DagPlan::from_items(vec![
                item("a", &[], PItemStatus::Failed(FailReason::BuildFailed)),
                item("b", &["a"], PItemStatus::WaitDeps),
                item("c", &["b"], PItemStatus::WaitDeps),
            ])
        };
        let mut p1 = make();
        let bare = p1.propagate_skip();
        let mut p2 = make();
        let rich: Vec<PItemId> = p2
            .propagate_skip_with_evidence(0, None)
            .into_iter()
            .map(|r| r.p_item_id)
            .collect();
        assert_eq!(bare, rich);
        assert_eq!(bare, vec!["b".to_string(), "c".to_string()]);
    }

    /// No failed ancestors → no skip records, nothing flipped.
    #[test]
    fn req007_no_failure_no_skip_records() {
        let mut plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Done),
            item("b", &["a"], PItemStatus::WaitDeps),
        ]);
        let records = plan.propagate_skip_with_evidence(5, Some("s".into()));
        assert!(records.is_empty());
        assert!(matches!(plan.get("b").unwrap().status, PItemStatus::WaitDeps));
    }

    #[test]
    fn serde_roundtrip_plan() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::WaitDeps),
            item("b", &["a"], PItemStatus::Done),
        ]);
        let json = serde_json::to_string(&plan).unwrap();
        let back: DagPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
    }
}
