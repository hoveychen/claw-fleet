import { describe, expect, it } from "vitest";
import type { PlanNode, TaskItem } from "../types";
import {
  PITCH_MAX,
  PITCH_MIN,
  ROW_CHROME,
  TITLE_MAX,
  TITLE_MIN,
  cellStates,
  matrixMetrics,
  matrixRows,
  nodeKey,
} from "./planMatrix";

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

describe("matrixMetrics", () => {
  const pitch = (m: { cellW: number; gap: number }) => m.cellW + m.gap;

  it("gives a wide window the roomy title and full-size cells", () => {
    const m = matrixMetrics(1400, 17);
    expect(m.titleW).toBe(TITLE_MAX);
    expect(pitch(m)).toBe(PITCH_MAX);
  });

  it("shrinks the title first, keeping cells full size", () => {
    const m = matrixMetrics(700, 17);
    expect(m.titleW).toBeLessThan(TITLE_MAX);
    expect(m.titleW).toBeGreaterThanOrEqual(TITLE_MIN);
    expect(pitch(m)).toBe(PITCH_MAX);
    // Still fits: title + strip + chrome is within the window.
    expect(m.titleW + 17 * pitch(m) + ROW_CHROME).toBeLessThanOrEqual(700);
  });

  it("only shrinks cells once the title has hit its floor", () => {
    const m = matrixMetrics(420, 17);
    expect(m.titleW).toBe(TITLE_MIN);
    expect(pitch(m)).toBeLessThan(PITCH_MAX);
    expect(pitch(m)).toBeGreaterThanOrEqual(PITCH_MIN);
  });

  it("stops at the floors and lets the container scroll", () => {
    const m = matrixMetrics(200, 60);
    expect(m.titleW).toBe(TITLE_MIN);
    expect(pitch(m)).toBe(PITCH_MIN);
  });

  it("keeps the gap proportional so tiny cells stay separable", () => {
    expect(matrixMetrics(1400, 17).gap).toBe(3);
    expect(matrixMetrics(200, 60).gap).toBe(1);
  });

  it("falls back to the roomy title when there are no cells at all", () => {
    expect(matrixMetrics(300, 0).titleW).toBe(TITLE_MAX);
  });
});
