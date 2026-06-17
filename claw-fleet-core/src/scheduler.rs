//! P-item scheduler helpers: which P-items can the orchestrator dispatch
//! right now, plus the "✨ auto-complete touches edges" suggestion engine.
//!
//! The scheduler is pure — `dispatchable()` is just the dependency-graph
//! unblocked set (resource scheduling was removed in the task-subsystem
//! rebuild; the deterministic orchestrator dispatches strictly by topology).
//!
//! Two responsibilities:
//! 1. `dispatchable()` — topology → candidate set.
//! 2. `suggest_touches_edges()` — UI "✨ auto-complete touches edges" button:
//!    surface P-items whose `touches` overlap (directly or via the implicit
//!    shared-file fallback list) without an existing edge.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::pitem::PItemId;
use crate::plan::DagPlan;

/// Files that almost any P-item in their domain touches but planners
/// commonly omit from `touches`. PRD patch §3.
///
/// Each entry is a glob-ish path fragment. The scheduler treats two P-items
/// as having "implicit overlap" if both `touches` contain a file matching
/// one of these fragments by suffix.
pub const IMPLICIT_SHARED_FILES: &[&str] = &[
    // Frontend
    "App.tsx",
    "App.ts",
    "App.jsx",
    "App.js",
    "Layout.tsx",
    "Layout.ts",
    "main.tsx",
    "main.ts",
    "index.tsx",
    "index.html",
    "package.json",
    // Rust
    "lib.rs",
    "main.rs",
    "mod.rs",
    "Cargo.toml",
    // Cross-cutting
    "CLAUDE.md",
];

/// One suggestion from `suggest_touches_edges`. The UI surfaces these as
/// "✨ auto-complete edges" — clicking adds `dependant` → depends_on `dep`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TouchesSuggestion {
    pub dependant: PItemId,
    pub dep: PItemId,
    pub shared_path: PathBuf,
    /// `true` if the overlap is via `IMPLICIT_SHARED_FILES` (the planner
    /// likely forgot to declare it) rather than a literal `touches` overlap.
    pub via_implicit_list: bool,
}

/// Dispatchable computation outcome. `error` is set when the plan has a
/// cycle (no items returned in that case).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dispatch {
    pub ready: Vec<PItemId>,
    pub error: Option<String>,
}

/// Compute the set of P-items the orchestrator should hand to workers right
/// now — every `WaitDeps` item whose dependencies are all settled. Selection
/// order is alphabetical by P-item id (via `unblocked_items`) so the same plan
/// state always yields the same dispatch.
///
/// On cycle: returns `ready: vec![]` with `error` describing the cycle so
/// the orchestrator can escalate to the user (don't silently freeze).
pub fn dispatchable(plan: &DagPlan) -> Dispatch {
    if let Some(cycle) = plan.cycle_detect() {
        return Dispatch {
            ready: vec![],
            error: Some(format!("plan contains a cycle: {cycle:?}")),
        };
    }
    Dispatch {
        ready: plan.unblocked_items(),
        error: None,
    }
}

/// Generate suggested implicit-ordering edges for the "✨ auto-complete
/// touches edges" UI button.
///
/// Two kinds of overlap surface as a suggestion:
/// 1. **Literal overlap** — two P-items list the same path in `touches`.
/// 2. **Implicit-list overlap** — two P-items each touch a file whose
///    basename (or suffix) matches an entry in `IMPLICIT_SHARED_FILES`.
///
/// For each overlapping pair `(a, b)` with no existing path between them
/// (in either direction), emit `(dependant: later, dep: earlier)` using
/// a stable ordering: the id that sorts smaller becomes the dep. This
/// guarantees consistent suggestions even when topology gives both
/// orderings equal merit; the human user can flip in the editor.
pub fn suggest_touches_edges(plan: &DagPlan) -> Vec<TouchesSuggestion> {
    let mut ids: Vec<&PItemId> = plan.items.keys().collect();
    ids.sort();
    let mut out: Vec<TouchesSuggestion> = Vec::new();
    for i in 0..ids.len() {
        for j in (i + 1)..ids.len() {
            let a = ids[i].clone();
            let b = ids[j].clone();
            if has_path(plan, &a, &b) || has_path(plan, &b, &a) {
                continue;
            }
            let Some(pa) = plan.get(&a) else { continue };
            let Some(pb) = plan.get(&b) else { continue };
            // Pick first overlap (literal first, then implicit).
            if let Some(shared) = literal_overlap(&pa.touches, &pb.touches) {
                out.push(TouchesSuggestion {
                    dependant: b.clone(),
                    dep: a.clone(),
                    shared_path: shared,
                    via_implicit_list: false,
                });
            } else if let Some(shared) = implicit_overlap(&pa.touches, &pb.touches) {
                out.push(TouchesSuggestion {
                    dependant: b.clone(),
                    dep: a.clone(),
                    shared_path: shared,
                    via_implicit_list: true,
                });
            }
        }
    }
    out
}

fn literal_overlap(a: &[PathBuf], b: &[PathBuf]) -> Option<PathBuf> {
    let set: HashSet<&Path> = a.iter().map(|p| p.as_path()).collect();
    b.iter().find(|p| set.contains(p.as_path())).cloned()
}

fn implicit_overlap(a: &[PathBuf], b: &[PathBuf]) -> Option<PathBuf> {
    let in_a: Vec<&PathBuf> = a.iter().filter(|p| matches_implicit(p)).collect();
    let in_b: Vec<&PathBuf> = b.iter().filter(|p| matches_implicit(p)).collect();
    for pa in &in_a {
        let a_name = file_basename(pa);
        for pb in &in_b {
            if a_name == file_basename(pb) {
                return Some((*pa).clone());
            }
        }
    }
    None
}

fn matches_implicit(p: &Path) -> bool {
    let name = match p.file_name().and_then(|s| s.to_str()) {
        Some(n) => n,
        None => return false,
    };
    IMPLICIT_SHARED_FILES.contains(&name)
        // Allow `locales/zh-CN.json` etc. via parent-dir check.
        || p.components().any(|c| c.as_os_str() == "locales")
        || p.components().any(|c| c.as_os_str() == "i18n")
}

fn file_basename(p: &Path) -> &str {
    p.file_name().and_then(|s| s.to_str()).unwrap_or("")
}

/// `true` if there's a directed path `from → ... → to` via `depends_on`.
/// Used to skip already-ordered pairs in `suggest_touches_edges`.
fn has_path(plan: &DagPlan, from: &PItemId, to: &PItemId) -> bool {
    if from == to {
        return true;
    }
    let mut stack = vec![from.clone()];
    let mut seen: HashSet<PItemId> = HashSet::new();
    while let Some(cur) = stack.pop() {
        if !seen.insert(cur.clone()) {
            continue;
        }
        let Some(p) = plan.get(&cur) else { continue };
        for d in &p.depends_on {
            if d == to {
                return true;
            }
            stack.push(d.clone());
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pitem::{FailReason, PItem, PItemStatus};

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

    fn item_touches(id: &str, deps: &[&str], touches: &[&str], status: PItemStatus) -> PItem {
        let mut p = item(id, deps, status);
        p.touches = touches.iter().map(PathBuf::from).collect();
        p
    }

    #[test]
    fn dispatches_unblocked_items() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::WaitDeps),
            item("b", &[], PItemStatus::WaitDeps),
        ]);
        let d = dispatchable(&plan);
        assert!(d.error.is_none());
        assert_eq!(d.ready, vec!["a".to_string(), "b".to_string()]);
    }

    #[test]
    fn waits_for_deps_done() {
        let plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Running),
            item("b", &["a"], PItemStatus::WaitDeps),
        ]);
        let d = dispatchable(&plan);
        assert!(d.ready.is_empty());
    }

    #[test]
    fn skip_propagation_unblocks_downstream() {
        // Only Failed poisons. Skipped should still unblock.
        let plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Skipped),
            item("b", &["a"], PItemStatus::WaitDeps),
        ]);
        let d = dispatchable(&plan);
        assert_eq!(d.ready, vec!["b".to_string()]);
    }

    #[test]
    fn failed_parent_keeps_child_in_waitdeps_until_propagate_skip_runs() {
        // Until `propagate_skip` flips the child to Skipped, it stays
        // WaitDeps and the scheduler refuses to dispatch (Failed parent
        // never satisfies `unblocks_downstream`).
        let mut plan = DagPlan::from_items(vec![
            item("a", &[], PItemStatus::Failed(FailReason::BuildFailed)),
            item("b", &["a"], PItemStatus::WaitDeps),
        ]);
        assert!(dispatchable(&plan).ready.is_empty());

        // Once propagate_skip flips 'b' to Skipped, it leaves WaitDeps and
        // no longer surfaces as dispatchable either.
        plan.propagate_skip();
        assert!(dispatchable(&plan).ready.is_empty());
        assert!(matches!(plan.get("b").unwrap().status, PItemStatus::Skipped));
    }

    #[test]
    fn cycle_returns_error_not_empty_silent() {
        let plan = DagPlan::from_items(vec![
            item("a", &["b"], PItemStatus::WaitDeps),
            item("b", &["a"], PItemStatus::WaitDeps),
        ]);
        let d = dispatchable(&plan);
        assert!(d.ready.is_empty());
        assert!(d.error.as_ref().is_some_and(|e| e.contains("cycle")));
    }

    #[test]
    fn suggest_edges_finds_literal_touches_overlap() {
        let plan = DagPlan::from_items(vec![
            item_touches("a", &[], &["src/foo.rs"], PItemStatus::WaitDeps),
            item_touches("b", &[], &["src/foo.rs"], PItemStatus::WaitDeps),
        ]);
        let s = suggest_touches_edges(&plan);
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].dep, "a");
        assert_eq!(s[0].dependant, "b");
        assert!(!s[0].via_implicit_list);
    }

    #[test]
    fn suggest_edges_via_implicit_shared_app_tsx() {
        let plan = DagPlan::from_items(vec![
            item_touches(
                "feat-a",
                &[],
                &["src/views/Feat.tsx", "src/App.tsx"],
                PItemStatus::WaitDeps,
            ),
            item_touches(
                "feat-b",
                &[],
                &["src/views/Other.tsx", "src/App.tsx"],
                PItemStatus::WaitDeps,
            ),
        ]);
        let s = suggest_touches_edges(&plan);
        assert_eq!(s.len(), 1, "expected one App.tsx overlap suggestion");
        // Literal overlap on App.tsx wins over implicit (it IS literally shared).
        assert!(!s[0].via_implicit_list);
    }

    #[test]
    fn suggest_edges_skips_pairs_already_ordered() {
        // a → b already; touches overlap but no suggestion needed.
        let plan = DagPlan::from_items(vec![
            item_touches("a", &[], &["src/foo.rs"], PItemStatus::WaitDeps),
            item_touches("b", &["a"], &["src/foo.rs"], PItemStatus::WaitDeps),
        ]);
        assert!(suggest_touches_edges(&plan).is_empty());
    }

    #[test]
    fn suggest_edges_implicit_list_locales() {
        let plan = DagPlan::from_items(vec![
            item_touches(
                "feat-a",
                &[],
                &["src/locales/en.json"],
                PItemStatus::WaitDeps,
            ),
            item_touches(
                "feat-b",
                &[],
                &["src/locales/en.json"],
                PItemStatus::WaitDeps,
            ),
        ]);
        let s = suggest_touches_edges(&plan);
        assert_eq!(s.len(), 1);
        // Literal — both touch en.json
        assert!(!s[0].via_implicit_list);
    }

    #[test]
    fn implicit_shared_list_includes_expected_files() {
        // Light contract test — the patch §3 list is intentionally short.
        assert!(IMPLICIT_SHARED_FILES.contains(&"App.tsx"));
        assert!(IMPLICIT_SHARED_FILES.contains(&"lib.rs"));
        assert!(IMPLICIT_SHARED_FILES.contains(&"Cargo.toml"));
        assert!(IMPLICIT_SHARED_FILES.contains(&"mod.rs"));
    }
}
