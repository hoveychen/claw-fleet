/* Layout for 计划树's node graph: a `parent`-linked plan forest laid out as a
   left-to-right tree. Pure geometry — no React, no DOM — so the awkward part
   (where a parent sits relative to a collapsed subtree) is unit-testable.

   Deliberately the simple tidy-tree pass, not Reingold-Tilford: every node in a
   column gets its own row, and a parent is centred between its first and last
   visible child. Real plan forests here are a handful of roots two or three
   deep, so the extra passes RT buys (packing sibling subtrees horizontally)
   would tighten nothing a reader would notice. */

import type { PlanNode } from "../types";

/** Node box. Wide enough for a CJK plan title before the ellipsis bites. */
export const NODE_W = 224;
export const NODE_H = 34;
/** Column pitch — box plus the gap the connector curve is drawn in. */
export const STEP_X = NODE_W + 52;
/** Row pitch between siblings, and the wider gap between whole root trees. */
export const GAP_Y = 12;
export const ROOT_GAP = 28;

/** Same `source:id` identity the old tree used for its React keys: one plan id
 *  may legitimately appear in two TASKS.md files (main checkout + a worktree),
 *  and those are different nodes. */
export function nodeKey(node: PlanNode): string {
  return `${node.source ?? ""}:${node.id}`;
}

export interface LayoutNode {
  node: PlanNode;
  key: string;
  depth: number;
  /** Top-left of the box, in stage coordinates. */
  x: number;
  y: number;
  hasChildren: boolean;
  collapsed: boolean;
  /** Plans hidden under this node while it is collapsed (0 when expanded). */
  hiddenDescendants: number;
}

export interface LayoutEdge {
  from: string;
  to: string;
  x1: number;
  y1: number;
  x2: number;
  y2: number;
}

export interface ForestLayout {
  nodes: LayoutNode[];
  edges: LayoutEdge[];
  width: number;
  height: number;
}

/** Plans below this node, collapsed or not. */
function descendantCount(node: PlanNode): number {
  return node.children.reduce((n, c) => n + 1 + descendantCount(c), 0);
}

/**
 * Lay the forest out. `collapsed` holds {@link nodeKey}s whose subtrees are
 * hidden; their descendants appear in neither `nodes` nor `edges`.
 */
export function layoutPlanForest(roots: PlanNode[], collapsed: Set<string>): ForestLayout {
  const nodes: LayoutNode[] = [];
  const edges: LayoutEdge[] = [];
  let cursorY = 0;
  let maxDepth = 0;

  /** Returns the laid-out node so the caller can centre itself on its children. */
  const walk = (node: PlanNode, depth: number): LayoutNode => {
    const key = nodeKey(node);
    const isCollapsed = collapsed.has(key);
    const hasChildren = node.children.length > 0;
    const showChildren = hasChildren && !isCollapsed;
    maxDepth = Math.max(maxDepth, depth);

    let y: number;
    let laidChildren: LayoutNode[] = [];
    if (showChildren) {
      laidChildren = node.children.map((c) => walk(c, depth + 1));
      const first = laidChildren[0];
      const last = laidChildren[laidChildren.length - 1];
      y = (first.y + last.y) / 2;
    } else {
      y = cursorY;
      cursorY += NODE_H + GAP_Y;
    }

    const laid: LayoutNode = {
      node,
      key,
      depth,
      x: depth * STEP_X,
      y,
      hasChildren,
      collapsed: isCollapsed,
      hiddenDescendants: isCollapsed ? descendantCount(node) : 0,
    };
    nodes.push(laid);
    for (const child of laidChildren) {
      edges.push({
        from: key,
        to: child.key,
        x1: laid.x + NODE_W,
        y1: laid.y + NODE_H / 2,
        x2: child.x,
        y2: child.y + NODE_H / 2,
      });
    }
    return laid;
  };

  roots.forEach((root, i) => {
    if (i > 0) cursorY += ROOT_GAP - GAP_Y;
    walk(root, 0);
  });

  const height = nodes.reduce((h, n) => Math.max(h, n.y + NODE_H), 0);
  return { nodes, edges, width: maxDepth * STEP_X + NODE_W, height };
}
