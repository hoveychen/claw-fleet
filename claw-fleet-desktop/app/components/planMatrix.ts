/* Row model for 计划树's progress matrix: one row per plan, one cell per
   P-task. Pure — no React, no DOM — so the ordering and the cell states are
   unit-testable away from the rendering.

   A matrix rather than a node graph because the data is not a tree: in this
   repo's own agent-workspace, 143 plans carry 9 `parent=` links, and of the 30
   with work left exactly one has a parent. Drawing a forest of singletons gives
   you a wall of identical boxes; laying the 222 P-tasks out in aligned columns
   gives you something to actually read across. The parent links that DO exist
   are kept as indentation. */

import type { PlanNode } from "../types";

/** `source:id` identity: one plan id may legitimately appear in two TASKS.md
 *  files (main checkout + a worktree), and those are different plans. */
export function nodeKey(node: PlanNode): string {
  return `${node.source ?? ""}:${node.id}`;
}

/** P-tasks left in this plan alone. */
export function pendingOf(node: PlanNode): number {
  return Math.max(0, node.total - node.done);
}

/** This plan or anything under it still has work. */
export function subtreeHasPending(node: PlanNode): boolean {
  return pendingOf(node) > 0 || node.children.some(subtreeHasPending);
}

export interface MatrixRow {
  node: PlanNode;
  key: string;
  depth: number;
  hasChildren: boolean;
  collapsed: boolean;
  /** Plans hidden under this row while it is collapsed (0 when expanded). */
  hiddenDescendants: number;
}

/** Plans below this node, collapsed or not. */
function descendantCount(node: PlanNode): number {
  return node.children.reduce((n, c) => n + 1 + descendantCount(c), 0);
}

/**
 * Flatten the forest into matrix rows.
 *
 * Roots are ordered by how much work is left (most first) so the top of the
 * page is the work that is actually open; children keep their TASKS.md order,
 * because a plan's P-tasks and its sub-plans read in the sequence they were
 * written. `collapsed` holds {@link nodeKey}s whose descendants are hidden.
 */
export function matrixRows(roots: PlanNode[], collapsed: Set<string>): MatrixRow[] {
  const out: MatrixRow[] = [];
  const walk = (node: PlanNode, depth: number) => {
    const key = nodeKey(node);
    const isCollapsed = collapsed.has(key);
    out.push({
      node,
      key,
      depth,
      hasChildren: node.children.length > 0,
      collapsed: isCollapsed,
      hiddenDescendants: isCollapsed ? descendantCount(node) : 0,
    });
    if (!isCollapsed) node.children.forEach((c) => walk(c, depth + 1));
  };
  [...roots]
    .sort((a, b) => pendingOf(b) - pendingOf(a))
    .forEach((r) => walk(r, 0));
  return out;
}

export type CellState = "done" | "next" | "todo";

/**
 * One cell per P-task, in TASKS.md order. `next` marks the first unchecked
 * item — the thing the plan is actually waiting on, and the only cell worth
 * picking out of a row of thirty.
 *
 * Falls back to the done/total counters when `items` disagrees with `total`:
 * the backend fills both, but a plan block whose checklist failed to parse
 * would otherwise render an empty row.
 */
export function cellStates(node: PlanNode): CellState[] {
  const flags =
    node.items.length === node.total
      ? node.items.map((i) => i.done)
      : Array.from({ length: node.total }, (_, i) => i < node.done);
  let markedNext = false;
  return flags.map((done) => {
    if (done) return "done";
    if (markedNext) return "todo";
    markedNext = true;
    return "next";
  });
}
