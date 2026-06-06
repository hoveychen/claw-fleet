//! `DagPlan` — task-level container for a set of `PItem` nodes plus the
//! plan-level operations the scheduler needs (topology, blocked-set computation,
//! resource-conflict detection, skip propagation).
//!
//! Pure DAG primitives live in `crate::dag`; this module specialises them for
//! `PItem` state. Defined per PRD §6.1 (design/task-as-unit-redesign.md).

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::dag;
use crate::pitem::{PItem, PItemId, PItemStatus, ResourceName};

// [REQ-002] Task-level container: an immutable-by-convention map of P-item id →
// P-item plus the accessors the scheduler needs. `node_ids()` gives a
// deterministic iteration order; `validate()` is the pre-run integrity gate
// (reference completeness + acyclicity).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DagPlan {
    pub items: HashMap<PItemId, PItem>,
}

/// A pair of items contending for the same resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceConflict {
    pub a: PItemId,
    pub b: PItemId,
    pub resource: ResourceName,
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
    // [REQ-002] Deterministic node iteration: the scheduler and traceability
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
    // [REQ-002] Pre-run integrity gate: every `depends_on` must resolve to a
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
    // [REQ-014] Returns the `WaitDeps` nodes whose `depends_on` are all terminal
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

    /// All pairs `(a, b, resource)` in `candidates` that contend for the same
    /// resource. Each *pair* appears at most once (`a < b`), even when the two
    /// items share several resources — the lexicographically-smallest shared
    /// resource is reported. The returned vec is ordered by `(a, b)` so the
    /// scheduler sees a deterministic, duplicate-free conflict set.
    // [REQ-015][REQ-049] Resource-conflict detection: the scheduler refuses to
    // run a conflicting pair concurrently. Pairs are de-duplicated and sorted so
    // two items contending on multiple resources don't double-count.
    pub fn resource_conflicts(&self, candidates: &[PItemId]) -> Vec<ResourceConflict> {
        let mut sorted: Vec<&PItemId> = candidates.iter().collect();
        sorted.sort();
        sorted.dedup();
        let mut out = Vec::new();
        for i in 0..sorted.len() {
            for j in (i + 1)..sorted.len() {
                let a = sorted[i];
                let b = sorted[j];
                let (Some(pa), Some(pb)) = (self.items.get(a), self.items.get(b)) else {
                    continue;
                };
                let set_a: HashSet<&ResourceName> = pa.resources.iter().collect();
                // Report the smallest shared resource so the pair is emitted at
                // most once and the choice is deterministic.
                let shared = pb
                    .resources
                    .iter()
                    .filter(|r| set_a.contains(*r))
                    .min();
                if let Some(r) = shared {
                    out.push(ResourceConflict {
                        a: a.clone(),
                        b: b.clone(),
                        resource: r.clone(),
                    });
                }
            }
        }
        out
    }

    /// Transitively flip every `WaitDeps` descendant of a `Failed` ancestor
    /// to `Skipped`. Already-terminal descendants are left alone. Returns the
    /// set of newly-skipped IDs.
    ///
    /// Convention: `Failed` poisons; `Skipped` and `Done` both unblock and do
    /// not poison. So a `Skipped` parent leaves children to run normally.
    pub fn propagate_skip(&mut self) -> Vec<PItemId> {
        // Forward adjacency: dep → [dependants]
        let mut dependants: HashMap<PItemId, Vec<PItemId>> = HashMap::new();
        for (id, p) in &self.items {
            for d in &p.depends_on {
                dependants.entry(d.clone()).or_default().push(id.clone());
            }
        }

        let mut newly_skipped: Vec<PItemId> = Vec::new();
        let mut frontier: Vec<PItemId> = self
            .items
            .iter()
            .filter(|(_, p)| matches!(p.status, PItemStatus::Failed(_)))
            .map(|(id, _)| id.clone())
            .collect();

        let mut visited: HashSet<PItemId> = HashSet::new();
        while let Some(node) = frontier.pop() {
            if !visited.insert(node.clone()) {
                continue;
            }
            if let Some(children) = dependants.get(&node) {
                for c in children.clone() {
                    let Some(child) = self.items.get_mut(&c) else {
                        continue;
                    };
                    if matches!(child.status, PItemStatus::WaitDeps | PItemStatus::WaitResource) {
                        child.status = PItemStatus::Skipped;
                        newly_skipped.push(c.clone());
                        frontier.push(c);
                    }
                }
            }
        }
        newly_skipped.sort();
        newly_skipped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitem::{FailReason, PItemStatus};

    fn item(id: &str, deps: &[&str], resources: &[&str], status: PItemStatus) -> PItem {
        PItem {
            id: id.into(),
            desc: id.into(),
            touches: vec![],
            depends_on: deps.iter().map(|s| (*s).to_string()).collect(),
            resources: resources.iter().map(|s| (*s).to_string()).collect(),
            estimate_secs: None,
            acceptance: vec![],
            artifacts: vec![],
            skippable: None,
            human_gate: false,
            status,
            agent_session_id: None,
            started_at: None,
            completed_at: None,
            output_summary: None,
        }
    }

    #[test]
    fn topo_sort_orders_deps_before_dependants() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], &[], PItemStatus::WaitDeps),
            item("b", &["a"], &[], PItemStatus::WaitDeps),
            item("c", &["b"], &[], PItemStatus::WaitDeps),
        ]);
        let order = plan.topo_sort().unwrap();
        let pos = |x: &str| order.iter().position(|s| s == x).unwrap();
        assert!(pos("a") < pos("b") && pos("b") < pos("c"));
    }

    #[test]
    fn cycle_detect_returns_witness() {
        let plan = DagPlan::from_items(vec![
            item("a", &["b"], &[], PItemStatus::WaitDeps),
            item("b", &["a"], &[], PItemStatus::WaitDeps),
        ]);
        let cycle = plan.cycle_detect().expect("cycle expected");
        assert_eq!(cycle.first(), cycle.last());
        assert!(plan.topo_sort().is_err());
    }

    #[test]
    fn validate_missing_dep() {
        let plan = DagPlan::from_items(vec![item("a", &["ghost"], &[], PItemStatus::WaitDeps)]);
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
            item("a", &["b"], &[], PItemStatus::WaitDeps),
            item("b", &["a"], &[], PItemStatus::WaitDeps),
        ]);
        assert!(matches!(
            plan.validate(),
            Err(PlanValidationError::Cycle { .. })
        ));
    }

    /// [REQ-002] `node_ids()` must be deterministic regardless of the order
    /// items were inserted into the underlying HashMap. Build the same plan two
    /// ways (forward + reversed insertion) and assert identical ordering.
    #[test]
    fn node_ids_is_deterministic_across_insertion_order() {
        let forward = DagPlan::from_items(vec![
            item("a", &[], &[], PItemStatus::WaitDeps),
            item("b", &[], &[], PItemStatus::WaitDeps),
            item("c", &[], &[], PItemStatus::WaitDeps),
        ]);
        let reversed = DagPlan::from_items(vec![
            item("c", &[], &[], PItemStatus::WaitDeps),
            item("b", &[], &[], PItemStatus::WaitDeps),
            item("a", &[], &[], PItemStatus::WaitDeps),
        ]);
        assert_eq!(forward.node_ids(), vec!["a", "b", "c"]);
        assert_eq!(forward.node_ids(), reversed.node_ids());
    }

    /// [REQ-002] Basic accessors exist and behave: get / get_mut / len.
    #[test]
    fn basic_accessors() {
        let mut plan = DagPlan::from_items(vec![item("a", &[], &[], PItemStatus::WaitDeps)]);
        assert_eq!(plan.len(), 1);
        assert!(plan.get("a").is_some());
        assert!(plan.get("missing").is_none());
        plan.get_mut("a").unwrap().desc = "edited".into();
        assert_eq!(plan.get("a").unwrap().desc, "edited");
    }

    /// [REQ-014] Registry scenario: nodes 2 and 3 both depend on 1. While 1 is
    /// WaitDeps nothing is unblocked; once 1 is Done both 2 and 3 unblock, in
    /// sorted order.
    #[test]
    fn unblocked_items_registry_scenario_1_unblocks_2_and_3() {
        let mut plan = DagPlan::from_items(vec![
            item("1", &[], &[], PItemStatus::WaitDeps),
            item("2", &["1"], &[], PItemStatus::WaitDeps),
            item("3", &["1"], &[], PItemStatus::WaitDeps),
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
            item("a", &[], &[], PItemStatus::Done),
            item("b", &["a"], &[], PItemStatus::WaitDeps),
            item("c", &["a", "b"], &[], PItemStatus::WaitDeps),
        ]);
        // Only "b" is unblocked — "c" still waits on "b".
        assert_eq!(plan.unblocked_items(), vec!["b".to_string()]);
    }

    #[test]
    fn unblocked_items_ignores_already_running() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], &[], PItemStatus::Done),
            item("b", &["a"], &[], PItemStatus::Running),
        ]);
        assert!(plan.unblocked_items().is_empty());
    }

    #[test]
    fn unblocked_items_treats_skipped_as_settled() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], &[], PItemStatus::Skipped),
            item("b", &["a"], &[], PItemStatus::WaitDeps),
        ]);
        assert_eq!(plan.unblocked_items(), vec!["b".to_string()]);
    }

    #[test]
    fn resource_conflicts_finds_shared_locks() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], &["build"], PItemStatus::WaitDeps),
            item("b", &[], &["build"], PItemStatus::WaitDeps),
            item("c", &[], &["test"], PItemStatus::WaitDeps),
        ]);
        let conflicts = plan.resource_conflicts(&["a".into(), "b".into(), "c".into()]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].a, "a");
        assert_eq!(conflicts[0].b, "b");
        assert_eq!(conflicts[0].resource, "build");
    }

    /// [REQ-015][REQ-049] Registry scenario: P1[build], P2[build], P3[git] →
    /// exactly one conflict (P1,P2 on build); P3 conflicts with no one.
    #[test]
    fn resource_conflicts_registry_scenario_p1_p2_on_build() {
        let plan = DagPlan::from_items(vec![
            item("P1", &[], &["build"], PItemStatus::WaitDeps),
            item("P2", &[], &["build"], PItemStatus::WaitDeps),
            item("P3", &[], &["git"], PItemStatus::WaitDeps),
        ]);
        let conflicts = plan.resource_conflicts(&["P1".into(), "P2".into(), "P3".into()]);
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].a, "P1");
        assert_eq!(conflicts[0].b, "P2");
        assert_eq!(conflicts[0].resource, "build");
    }

    /// [REQ-015] Two items sharing *multiple* resources must yield a single
    /// conflict entry for the pair (deduplicated), not one per shared resource.
    #[test]
    fn resource_conflicts_dedups_pairs_sharing_multiple_resources() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], &["build", "test"], PItemStatus::WaitDeps),
            item("b", &[], &["test", "build"], PItemStatus::WaitDeps),
        ]);
        let conflicts = plan.resource_conflicts(&["a".into(), "b".into()]);
        assert_eq!(conflicts.len(), 1, "pair must appear once, not once per shared resource");
        assert_eq!(conflicts[0].a, "a");
        assert_eq!(conflicts[0].b, "b");
        // Smallest shared resource is reported deterministically.
        assert_eq!(conflicts[0].resource, "build");
    }

    /// [REQ-015] Output is ordered by (a, b) regardless of candidate input order.
    #[test]
    fn resource_conflicts_ordered_by_pair() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], &["build"], PItemStatus::WaitDeps),
            item("b", &[], &["build"], PItemStatus::WaitDeps),
            item("c", &[], &["build"], PItemStatus::WaitDeps),
        ]);
        // Feed candidates in scrambled order.
        let conflicts = plan.resource_conflicts(&["c".into(), "a".into(), "b".into()]);
        let pairs: Vec<(&str, &str)> = conflicts
            .iter()
            .map(|c| (c.a.as_str(), c.b.as_str()))
            .collect();
        assert_eq!(pairs, vec![("a", "b"), ("a", "c"), ("b", "c")]);
    }

    #[test]
    fn resource_conflicts_empty_when_disjoint() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], &["build"], PItemStatus::WaitDeps),
            item("b", &[], &["test"], PItemStatus::WaitDeps),
        ]);
        assert!(plan
            .resource_conflicts(&["a".into(), "b".into()])
            .is_empty());
    }

    #[test]
    fn propagate_skip_marks_transitive_descendants() {
        let mut plan = DagPlan::from_items(vec![
            item("a", &[], &[], PItemStatus::Failed(FailReason::BuildFailed)),
            item("b", &["a"], &[], PItemStatus::WaitDeps),
            item("c", &["b"], &[], PItemStatus::WaitDeps),
            // Unrelated chain — should stay WaitDeps.
            item("x", &[], &[], PItemStatus::Done),
            item("y", &["x"], &[], PItemStatus::WaitDeps),
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
            item("a", &[], &[], PItemStatus::Failed(FailReason::BuildFailed)),
            // Already Done — don't touch.
            item("b", &["a"], &[], PItemStatus::Done),
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
            item("a", &[], &[], PItemStatus::Skipped),
            item("b", &["a"], &[], PItemStatus::WaitDeps),
        ]);
        let skipped = plan.propagate_skip();
        assert!(skipped.is_empty());
        assert!(matches!(plan.get("b").unwrap().status, PItemStatus::WaitDeps));
    }

    #[test]
    fn serde_roundtrip_plan() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], &["build"], PItemStatus::WaitDeps),
            item("b", &["a"], &[], PItemStatus::Done),
        ]);
        let json = serde_json::to_string(&plan).unwrap();
        let back: DagPlan = serde_json::from_str(&json).unwrap();
        assert_eq!(plan, back);
    }
}
