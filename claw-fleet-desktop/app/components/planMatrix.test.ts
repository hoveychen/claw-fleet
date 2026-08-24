import { describe, expect, it } from "vitest";
import type { PlanNode, TaskItem } from "../types";
import { cellStates, matrixRows, nodeKey } from "./planMatrix";

function plan(
  id: string,
  opts: { children?: PlanNode[]; items?: TaskItem[]; done?: number; total?: number } = {},
): PlanNode {
  const items = opts.items ?? [];
  return {
    id,
    title: id,
    source: null,
    kind: "exec",
    items,
    done: opts.done ?? items.filter((i) => i.done).length,
    total: opts.total ?? items.length,
    chains: [],
    children: opts.children ?? [],
    orphanedParent: null,
  };
}

const item = (done: boolean): TaskItem => ({ text: "x", done });

describe("matrixRows", () => {
  it("orders roots by pending work, most first", () => {
    const roots = [
      plan("almost", { items: [item(true), item(false)] }),
      plan("fresh", { items: [item(false), item(false), item(false)] }),
    ];
    expect(matrixRows(roots, new Set()).map((r) => r.node.id)).toEqual(["fresh", "almost"]);
  });

  it("indents children under their parent and keeps their file order", () => {
    const root = plan("root", {
      items: [item(false)],
      children: [plan("b", { items: [item(false), item(false)] }), plan("a", { items: [item(false)] })],
    });
    const rows = matrixRows([root], new Set());
    expect(rows.map((r) => [r.node.id, r.depth])).toEqual([
      ["root", 0],
      ["b", 1],
      ["a", 1],
    ]);
    expect(rows[0].hasChildren).toBe(true);
    expect(rows[1].hasChildren).toBe(false);
  });

  it("hides a collapsed row's descendants and reports how many", () => {
    const child = plan("child", { items: [item(false)], children: [plan("grand", { items: [item(false)] })] });
    const root = plan("root", { items: [item(false)], children: [child] });
    const rows = matrixRows([root], new Set([nodeKey(child)]));
    expect(rows.map((r) => r.node.id)).toEqual(["root", "child"]);
    expect(rows[1]).toMatchObject({ collapsed: true, hasChildren: true, hiddenDescendants: 1 });
  });
});

describe("cellStates", () => {
  it("marks only the first unchecked item as next", () => {
    const p = plan("p", { items: [item(true), item(false), item(false)] });
    expect(cellStates(p)).toEqual(["done", "next", "todo"]);
  });

  it("keeps TASKS.md order when a later item was ticked first", () => {
    const p = plan("p", { items: [item(false), item(true), item(false)] });
    expect(cellStates(p)).toEqual(["next", "done", "todo"]);
  });

  it("has no next cell once everything is done", () => {
    const p = plan("p", { items: [item(true), item(true)] });
    expect(cellStates(p)).toEqual(["done", "done"]);
  });

  it("falls back to the counters when items do not match total", () => {
    const p = plan("p", { items: [], done: 2, total: 4 });
    expect(cellStates(p)).toEqual(["done", "done", "next", "todo"]);
  });
});
