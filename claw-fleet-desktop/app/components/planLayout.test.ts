import { describe, expect, it } from "vitest";
import type { PlanNode } from "../types";
import {
  COL_GAP,
  GAP_Y,
  NODE_H,
  NODE_W,
  ROOT_GAP,
  STEP_X,
  layoutPlanForest,
  nodeKey,
} from "./planLayout";

/** Narrow enough that every root tree lands on its own band. */
const ONE_PER_BAND = NODE_W;

function plan(id: string, children: PlanNode[] = [], done = 0, total = 1): PlanNode {
  return {
    id,
    title: id,
    source: null,
    kind: "exec",
    items: [],
    done,
    total,
    chains: [],
    children,
    orphanedParent: null,
  };
}

describe("layoutPlanForest", () => {
  it("places a lone root at the origin and sizes the stage to it", () => {
    const l = layoutPlanForest([plan("a")], new Set(), ONE_PER_BAND);
    expect(l.nodes).toHaveLength(1);
    expect(l.nodes[0]).toMatchObject({ key: nodeKey(plan("a")), depth: 0, x: 0, y: 0 });
    expect(l.edges).toHaveLength(0);
    expect(l.width).toBe(NODE_W);
    expect(l.height).toBe(NODE_H);
  });

  it("stacks children in one column and centres the parent on them", () => {
    const root = plan("root", [plan("c1"), plan("c2")]);
    const l = layoutPlanForest([root], new Set(), Infinity);

    const c1 = l.nodes.find((n) => n.node.id === "c1")!;
    const c2 = l.nodes.find((n) => n.node.id === "c2")!;
    const r = l.nodes.find((n) => n.node.id === "root")!;

    expect(c1.depth).toBe(1);
    expect(c1.x).toBe(STEP_X);
    expect(c1.y).toBe(0);
    expect(c2.y).toBe(NODE_H + GAP_Y);
    // Parent sits halfway between its first and last child.
    expect(r.y).toBe((c1.y + c2.y) / 2);
    expect(r.hasChildren).toBe(true);
    expect(l.width).toBe(STEP_X + NODE_W);
  });

  it("emits one edge per parent→child link, anchored mid-height", () => {
    const root = plan("root", [plan("c1")]);
    const l = layoutPlanForest([root], new Set(), Infinity);
    const r = l.nodes.find((n) => n.node.id === "root")!;
    const c1 = l.nodes.find((n) => n.node.id === "c1")!;

    expect(l.edges).toHaveLength(1);
    expect(l.edges[0]).toMatchObject({
      x1: r.x + NODE_W,
      y1: r.y + NODE_H / 2,
      x2: c1.x,
      y2: c1.y + NODE_H / 2,
    });
  });

  it("drops a collapsed node's whole subtree from nodes and edges", () => {
    const root = plan("root", [plan("c1", [plan("g1")]), plan("c2")]);
    const l = layoutPlanForest([root], new Set([nodeKey(plan("c1"))]), Infinity);

    expect(l.nodes.map((n) => n.node.id).sort()).toEqual(["c1", "c2", "root"]);
    expect(l.edges).toHaveLength(2);
    // The collapsed node still reports the subtree it is hiding.
    const c1 = l.nodes.find((n) => n.node.id === "c1")!;
    expect(c1).toMatchObject({ collapsed: true, hasChildren: true, hiddenDescendants: 1 });
  });

  it("separates sibling roots by the wider root gap", () => {
    const l = layoutPlanForest([plan("a"), plan("b")], new Set(), ONE_PER_BAND);
    const a = l.nodes.find((n) => n.node.id === "a")!;
    const b = l.nodes.find((n) => n.node.id === "b")!;
    expect(b.y).toBe(a.y + NODE_H + ROOT_GAP);
    expect(l.height).toBe(b.y + NODE_H);
  });

  it("packs root trees side by side until the band is full", () => {
    const roots = [plan("a"), plan("b"), plan("c")];
    // Room for two boxes plus the gap between them, not three.
    const l = layoutPlanForest(roots, new Set(), NODE_W * 2 + COL_GAP);
    const at = (id: string) => l.nodes.find((n) => n.node.id === id)!;

    expect(at("a")).toMatchObject({ x: 0, y: 0 });
    expect(at("b")).toMatchObject({ x: NODE_W + COL_GAP, y: 0 });
    // Third tree wraps onto the next band.
    expect(at("c")).toMatchObject({ x: 0, y: NODE_H + ROOT_GAP });
    expect(l.width).toBe(NODE_W * 2 + COL_GAP);
    expect(l.height).toBe(NODE_H * 2 + ROOT_GAP);
  });

  it("gives a deep tree its own full-width block on the band", () => {
    const deep = plan("deep", [plan("kid")]);
    const l = layoutPlanForest([deep, plan("flat")], new Set(), Infinity);
    const flat = l.nodes.find((n) => n.node.id === "flat")!;
    // The deep tree is two columns wide, so its neighbour starts past both.
    expect(flat.x).toBe(STEP_X + NODE_W + COL_GAP);
    expect(flat.y).toBe(0);
  });

  it("keys nodes by source + id so the same id in two TASKS.md files stays distinct", () => {
    const a = { ...plan("dup"), source: ".worktrees/x/TASKS.md" };
    const b = plan("dup");
    const l = layoutPlanForest([a, b], new Set(), ONE_PER_BAND);
    expect(new Set(l.nodes.map((n) => n.key)).size).toBe(2);
  });
});
