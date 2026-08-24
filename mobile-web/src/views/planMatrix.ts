/* Row model for the phone's 计划 page: one row per plan, one cell per P-task.
   Pure — no React, no DOM.

   Deliberate copy of claw-fleet-desktop/app/components/planMatrix.ts (the two
   apps are separate packages with no shared module; the desktop file carries
   the same note). Keep the row ordering and cell states identical — only the
   width constants differ, because a phone is not a 1500px window. */

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

// ── Responsive metrics ───────────────────────────────────────────────────────

/** Everything on a row that is neither the title nor the cells: the caret hit
 *  area, the relay slot, the done/total counter, the flex gaps, the strip's
 *  left margin and the row padding. Measured against the rendered row, not
 *  guessed — an under-estimate here shows up as a scrollbar the metrics said
 *  would not be needed. */
export const ROW_CHROME = 74;
/** Roomy end of the scale — what a wide window gets. */
export const TITLE_MAX = 200;
export const PITCH_MAX = 11;
/** Floors. Past these the matrix scrolls horizontally rather than shrink into
 *  illegibility: a 3px cell is a smudge and a 100px title is not a title. */
export const TITLE_MIN = 96;
export const PITCH_MIN = 5;

export interface MatrixMetrics {
  /** Width of the fixed title column — the thing that keeps columns aligned. */
  titleW: number;
  /** Cell box and the gap after it; `titleW + n * (cellW + gap)` is the row. */
  cellW: number;
  gap: number;
}

/**
 * Fit the widest row into `availWidth`.
 *
 * The title yields first (420 → 140) because a shrunken *cell* costs the view
 * its whole point — you read progress down the columns — while a clipped title
 * still has the drawer and a tooltip behind it. Only once the title is at its
 * floor do the cells start shrinking, and below `PITCH_MIN` we stop and let the
 * container scroll instead.
 */
export function matrixMetrics(availWidth: number, maxTotal: number): MatrixMetrics {
  const pitchToParts = (pitch: number) => {
    const gap = pitch >= 10 ? 3 : pitch >= 8 ? 2 : 1;
    return { cellW: pitch - gap, gap };
  };
  const avail = Math.max(0, availWidth - ROW_CHROME);
  if (maxTotal <= 0) return { titleW: TITLE_MAX, ...pitchToParts(PITCH_MAX) };

  const titleAtFullPitch = avail - maxTotal * PITCH_MAX;
  if (titleAtFullPitch >= TITLE_MIN) {
    return {
      titleW: Math.min(TITLE_MAX, Math.floor(titleAtFullPitch)),
      ...pitchToParts(PITCH_MAX),
    };
  }
  const pitch = Math.max(
    PITCH_MIN,
    Math.min(PITCH_MAX, Math.floor((avail - TITLE_MIN) / maxTotal)),
  );
  return { titleW: TITLE_MIN, ...pitchToParts(pitch) };
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

/** Split a TASKS.md item into its `P<n>` marker and the rest. Items are written
 *  `**P3** — do the thing`, so raw they render as literal asterisks — the
 *  P-number, the one part you scan by, looks like noise. Anything that does not
 *  match keeps its text verbatim rather than being mangled by a half-baked
 *  markdown pass. */
export function splitMarker(text: string): { marker: string | null; rest: string } {
  const m = /^\*\*(P\d+[a-z]?)\*\*\s*(?:[—–-]\s*)?([\s\S]*)$/.exec(text.trim());
  if (!m) return { marker: null, rest: text };
  return { marker: m[1], rest: m[2].replace(/\*\*/g, "") };
}
