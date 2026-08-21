//! Plan forest — the whole execution chain of a workspace as one structure:
//! the `parent`-linked plan tree, with each plan's handoff relay chains folded
//! onto the plan node they belong to.
//!
//! Why this is one aggregate rather than two lookups: the plan tree
//! (`parent="..."` sentinels) and the relay chains (`HandoffChain.plan_id`) are
//! stored independently and were never joined anywhere, so nothing in the UI
//! could show "this plan was executed across four relayed sessions". The join
//! key already exists on both sides — this module performs it once, server-side,
//! so the desktop and any remote client render from the same shape.
//!
//! Two deliberate inclusions, because the point is to see the *whole* chain:
//! - **Completed plans are kept.** Folding/filtering them is the view's job;
//!   dropping them here would make history unreachable.
//! - **Unattached chains are kept.** A `fleet handoff` with no `--plan` is a
//!   real relay that really happened; omitting it would silently lose work.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::path::Path;

use crate::handoff::HandoffChain;
use crate::prd_tasks::{self as pt, PlanKind, TaskItem};

/// One plan in the forest: its own state, the relays that executed it, and its
/// child plans.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PlanNode {
    /// Sentinel id. Always present — a node must be addressable to be a tree
    /// member, so legacy anonymous blocks are counted separately instead (see
    /// [`PlanForest::anonymous`]).
    pub id: String,
    pub title: Option<String>,
    /// Relative path to the worktree TASKS.md this plan came from; `None` for
    /// the main checkout (the implicit norm).
    pub source: Option<String>,
    pub kind: PlanKind,
    pub items: Vec<TaskItem>,
    pub done: u32,
    pub total: u32,
    /// Relay chains whose `plan_id` names this plan — the "接力 n/N" the view
    /// folds into a single node.
    pub chains: Vec<HandoffChain>,
    pub children: Vec<PlanNode>,
    /// Set when this plan declared a `parent` that is nowhere in the workspace,
    /// so it was promoted to a root. Surfaced rather than hidden: a dangling
    /// parent means someone deleted or renamed a plan block, and the view should
    /// be able to say so.
    pub orphaned_parent: Option<String>,
}

impl PlanNode {
    /// Pending top-level P-tasks in this plan alone (children excluded).
    pub fn pending(&self) -> u32 {
        self.total.saturating_sub(self.done)
    }
}

/// Every plan in a workspace, as roots-with-descendants, plus the leftovers
/// that cannot hang on the tree.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "ts-export", derive(ts_rs::TS))]
#[serde(rename_all = "camelCase")]
pub struct PlanForest {
    pub roots: Vec<PlanNode>,
    /// Relay chains with no `plan_id`, or one naming a plan that isn't here.
    /// A free-form `fleet handoff` still represents real executed work.
    pub unattached_chains: Vec<HandoffChain>,
    /// Count of legacy anonymous (no-`id`) blocks seen. They predate the v2
    /// sentinel and cannot participate in a tree, but the view should be able to
    /// tell the user they exist rather than silently under-reporting.
    pub anonymous: u32,
}

/// Build the forest for the workspace containing `cwd`, joining in `chains`.
///
/// `chains` is passed in rather than read here so the caller controls the source
/// (all chains vs. one workspace's) and tests stay filesystem-light.
pub fn build(cwd: &Path, chains: Vec<HandoffChain>) -> PlanForest {
    let main_root = pt::discover_main_checkout_root(cwd);
    let sources = pt::collect_task_sources(cwd, main_root.as_deref());
    // `false` = keep completed plans: the whole chain includes finished work.
    let (raw, _) = pt::collect_from_sources(&sources, false);
    let blocks = pt::dedup_blocks_keep_latest_mtime(raw);
    build_from(&blocks, chains, main_root.as_deref())
}

/// Pure core of [`build`]: assemble the forest from already-loaded blocks.
/// Split out so the tree-shaping rules (orphans, cycles, chain attachment) are
/// testable without a filesystem.
pub fn build_from(
    blocks: &[pt::SourcedBlock],
    chains: Vec<HandoffChain>,
    main_root: Option<&Path>,
) -> PlanForest {
    let primary = main_root.map(|r| r.join("TASKS.md"));
    let anonymous = blocks.iter().filter(|b| b.id.is_none()).count() as u32;

    // Group chains by the plan they name, so each node can take its own.
    let mut chains_by_plan: HashMap<String, Vec<HandoffChain>> = HashMap::new();
    let mut unattached: Vec<HandoffChain> = Vec::new();
    let known: HashSet<&str> = blocks.iter().filter_map(|b| b.id.as_deref()).collect();
    for c in chains {
        match c.plan_id.as_deref() {
            Some(p) if known.contains(p) => {
                chains_by_plan.entry(p.to_string()).or_default().push(c)
            }
            _ => unattached.push(c),
        }
    }

    // Flat nodes first, in file order — that order is carried into `children`
    // and `roots`, so the view reads top-to-bottom like the file does.
    let mut order: Vec<String> = Vec::new();
    let mut nodes: HashMap<String, PlanNode> = HashMap::new();
    let mut declared_parent: HashMap<String, Option<String>> = HashMap::new();
    for b in blocks {
        let Some(id) = b.id.as_deref() else { continue };
        if nodes.contains_key(id) {
            continue; // dedup already picked a winner; ignore a stray repeat
        }
        let (done, total) = pt::count_tasks(&b.body);
        order.push(id.to_string());
        declared_parent.insert(id.to_string(), b.parent.clone());
        nodes.insert(
            id.to_string(),
            PlanNode {
                id: id.to_string(),
                title: pt::extract_plan_name(&b.body),
                source: pt::display_source(&b.source, main_root, primary.as_deref()),
                kind: b.kind,
                items: pt::parse_task_items(&b.body),
                done,
                total,
                chains: chains_by_plan.remove(id).unwrap_or_default(),
                children: Vec::new(),
                orphaned_parent: None,
            },
        );
    }

    // Decide each node's effective parent: a declared parent that exists and
    // does not close a cycle. Everything else becomes a root, and a *dangling*
    // parent is recorded on the node so the view can flag it.
    let mut effective_parent: HashMap<String, String> = HashMap::new();
    for id in &order {
        let Some(Some(declared)) = declared_parent.get(id) else {
            continue;
        };
        if !nodes.contains_key(declared) {
            if let Some(n) = nodes.get_mut(id) {
                n.orphaned_parent = Some(declared.clone());
            }
            continue;
        }
        if closes_cycle(id, declared, &declared_parent) {
            continue; // treat as root rather than lose the node to a cycle
        }
        effective_parent.insert(id.clone(), declared.clone());
    }

    // Attach bottom-up: build each parent's child list, then move whole subtrees
    // into place. Walking `order` in reverse means a child is always already
    // assembled when its parent takes ownership of it.
    let mut children_of: HashMap<String, Vec<String>> = HashMap::new();
    for id in &order {
        if let Some(p) = effective_parent.get(id) {
            children_of.entry(p.clone()).or_default().push(id.clone());
        }
    }
    let roots: Vec<String> = order
        .iter()
        .filter(|id| !effective_parent.contains_key(*id))
        .cloned()
        .collect();

    let roots = roots
        .into_iter()
        .filter_map(|id| take_subtree(&id, &mut nodes, &children_of))
        .collect();

    PlanForest {
        roots,
        unattached_chains: unattached,
        anonymous,
    }
}

/// Remove `id` from `nodes` with its descendants attached, depth-first.
fn take_subtree(
    id: &str,
    nodes: &mut HashMap<String, PlanNode>,
    children_of: &HashMap<String, Vec<String>>,
) -> Option<PlanNode> {
    let mut node = nodes.remove(id)?;
    if let Some(kids) = children_of.get(id) {
        for k in kids {
            if let Some(child) = take_subtree(k, nodes, children_of) {
                node.children.push(child);
            }
        }
    }
    Some(node)
}

/// Would making `parent` the parent of `id` close a cycle? True when `id` is
/// reachable by walking `parent`'s own declared-parent chain (including
/// `parent == id`, a self-loop).
fn closes_cycle(
    id: &str,
    parent: &str,
    declared: &HashMap<String, Option<String>>,
) -> bool {
    let mut seen: HashSet<&str> = HashSet::new();
    let mut cursor = Some(parent);
    while let Some(cur) = cursor {
        if cur == id {
            return true;
        }
        if !seen.insert(cur) {
            return true; // a pre-existing cycle upstream; don't walk it forever
        }
        cursor = declared.get(cur).and_then(|p| p.as_deref());
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::handoff::{HandoffChain, HandoffLink};
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    fn block(id: Option<&str>, body: &str, parent: Option<&str>) -> pt::SourcedBlock {
        pt::SourcedBlock {
            id: id.map(|s| s.to_string()),
            body: body.to_string(),
            source: PathBuf::from("/r/TASKS.md"),
            mtime: UNIX_EPOCH,
            parent: parent.map(|s| s.to_string()),
            kind: PlanKind::Exec,
        }
    }

    fn chain(id: &str, plan: Option<&str>, hops: u32) -> HandoffChain {
        let links = (0..hops)
            .map(|i| HandoffLink {
                from_session_id: format!("s{i}"),
                to_session_id: format!("s{}", i + 1),
                note: format!("leg {i}"),
                handed_at: 1_000 + i as u64,
                plan_id: plan.map(|s| s.to_string()),
                next_task: None,
            })
            .collect();
        HandoffChain {
            chain_id: id.to_string(),
            workspace_path: "/r".to_string(),
            plan_id: plan.map(|s| s.to_string()),
            links,
        }
    }

    /// The shape the whole feature exists for: a parent/child/grandchild chain
    /// nests, and each plan's relays land on its own node.
    #[test]
    fn builds_nested_tree_with_chains_on_their_own_plans() {
        let blocks = [
            block(Some("root"), "**Plan:** Root\n\n- [x] **P1** — a\n- [ ] **P2** — b\n", None),
            block(Some("mid"), "**Plan:** Mid\n\n- [ ] **P1** — m\n", Some("root")),
            block(Some("leaf"), "**Plan:** Leaf\n\n- [x] **P1** — l\n", Some("mid")),
        ];
        let forest = build_from(
            &blocks,
            vec![chain("c1", Some("mid"), 3), chain("c2", Some("root"), 1)],
            None,
        );

        assert_eq!(forest.roots.len(), 1, "one root: {:?}", forest.roots);
        let root = &forest.roots[0];
        assert_eq!(root.id, "root");
        assert_eq!((root.done, root.total), (1, 2));
        assert_eq!(root.pending(), 1);
        assert_eq!(root.chains.len(), 1, "root's own chain");
        assert_eq!(root.chains[0].chain_id, "c2");

        let mid = &root.children[0];
        assert_eq!(mid.id, "mid");
        assert_eq!(mid.chains.len(), 1);
        assert_eq!(mid.chains[0].links.len(), 3, "3-hop relay folds onto mid");

        assert_eq!(mid.children[0].id, "leaf");
        assert!(forest.unattached_chains.is_empty());
    }

    /// A chain with no plan, or one naming a plan that isn't here, must survive
    /// as unattached rather than vanish.
    #[test]
    fn keeps_chains_that_cannot_attach() {
        let blocks = [block(Some("a"), "- [ ] **P1** — x\n", None)];
        let forest = build_from(
            &blocks,
            vec![
                chain("free", None, 2),
                chain("ghost", Some("deleted-plan"), 1),
                chain("ok", Some("a"), 1),
            ],
            None,
        );
        assert_eq!(forest.roots[0].chains.len(), 1);
        let mut unattached: Vec<&str> =
            forest.unattached_chains.iter().map(|c| c.chain_id.as_str()).collect();
        unattached.sort();
        assert_eq!(unattached, vec!["free", "ghost"]);
    }

    /// A plan whose declared parent is gone becomes a root and says so, instead
    /// of disappearing from the forest.
    #[test]
    fn dangling_parent_is_promoted_to_root_and_flagged() {
        let blocks = [
            block(Some("kid"), "- [ ] **P1** — k\n", Some("vanished")),
            block(Some("solo"), "- [ ] **P1** — s\n", None),
        ];
        let forest = build_from(&blocks, vec![], None);
        assert_eq!(forest.roots.len(), 2);
        let kid = forest.roots.iter().find(|n| n.id == "kid").unwrap();
        assert_eq!(kid.orphaned_parent.as_deref(), Some("vanished"));
        let solo = forest.roots.iter().find(|n| n.id == "solo").unwrap();
        assert_eq!(solo.orphaned_parent, None);
    }

    /// A parent cycle must not hang the build or drop the nodes: both become
    /// roots so every plan is still reachable in the view.
    #[test]
    fn parent_cycle_degrades_to_roots_without_hanging() {
        let blocks = [
            block(Some("x"), "- [ ] **P1** — x\n", Some("y")),
            block(Some("y"), "- [ ] **P1** — y\n", Some("x")),
        ];
        let forest = build_from(&blocks, vec![], None);
        let ids: HashSet<&str> = forest.roots.iter().map(|n| n.id.as_str()).collect();
        assert!(ids.contains("x") && ids.contains("y"), "{ids:?}");
    }

    /// A self-parent is the degenerate cycle.
    #[test]
    fn self_parent_becomes_a_root() {
        let blocks = [block(Some("me"), "- [ ] **P1** — m\n", Some("me"))];
        let forest = build_from(&blocks, vec![], None);
        assert_eq!(forest.roots.len(), 1);
        assert_eq!(forest.roots[0].id, "me");
    }

    /// Completed plans stay in the forest — the view needs history to show the
    /// whole execution chain — and legacy anonymous blocks are counted, not
    /// silently dropped.
    #[test]
    fn keeps_completed_plans_and_counts_anonymous_blocks() {
        let blocks = [
            block(Some("done"), "**Plan:** Done\n\n- [x] **P1** — d\n", None),
            block(None, "legacy free-form PRD text\n", None),
            block(None, "another legacy block\n", None),
        ];
        let forest = build_from(&blocks, vec![], None);
        assert_eq!(forest.roots.len(), 1, "anonymous blocks are not tree members");
        assert_eq!(forest.roots[0].id, "done");
        assert_eq!(forest.roots[0].pending(), 0);
        assert_eq!(forest.anonymous, 2);
    }

    /// Roots and children preserve TASKS.md order, so the view reads like the
    /// file rather than in hash order.
    #[test]
    fn preserves_file_order_for_roots_and_children() {
        let blocks = [
            block(Some("r1"), "- [ ] **P1** — a\n", None),
            block(Some("c2"), "- [ ] **P1** — b\n", Some("r1")),
            block(Some("r2"), "- [ ] **P1** — c\n", None),
            block(Some("c1"), "- [ ] **P1** — d\n", Some("r1")),
        ];
        let forest = build_from(&blocks, vec![], None);
        let roots: Vec<&str> = forest.roots.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(roots, vec!["r1", "r2"]);
        let kids: Vec<&str> = forest.roots[0].children.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(kids, vec!["c2", "c1"], "children keep file order, not sorted");
    }
}
