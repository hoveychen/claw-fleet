/**
 * Rows of a rendered diff, and the conversion from Claude Code's real
 * `structuredPatch` hunks into them.
 *
 * Split out of `DiffView` so the line-number arithmetic can be exercised
 * against real transcripts without dragging a React component (and its CSS
 * module) into a test harness. Getting this wrong shifts every line number in
 * the gutter, which no type checker would catch.
 */

import type { PatchHunk } from "./toolResults";

export type DiffLine =
  | { kind: "ctx"; oldLine: number; newLine: number; text: string }
  | { kind: "del"; oldLine: number; text: string }
  | { kind: "add"; newLine: number; text: string };

export type Row = DiffLine | { kind: "sep" };

/**
 * Expand `structuredPatch` hunks into rows, separating each hunk with the same
 * `⋯` marker the LCS path uses.
 *
 * Each entry in `lines` is prefixed `+`, `-` or a space. Real diffs also carry
 * git's `\ No newline at end of file` marker, which describes the previous line
 * rather than being a line of the file — counting it would desynchronise both
 * line counters from that point on.
 */
export function rowsFromHunks(hunks: PatchHunk[]): Row[] {
  const out: Row[] = [];
  hunks.forEach((hunk, hi) => {
    if (hi > 0) out.push({ kind: "sep" });
    let oldLine = hunk.oldStart;
    let newLine = hunk.newStart;
    for (const line of hunk.lines) {
      if (line.startsWith("\\")) continue;
      const text = line.slice(1);
      if (line.startsWith("+")) {
        out.push({ kind: "add", newLine: newLine++, text });
      } else if (line.startsWith("-")) {
        out.push({ kind: "del", oldLine: oldLine++, text });
      } else {
        out.push({ kind: "ctx", oldLine: oldLine++, newLine: newLine++, text });
      }
    }
  });
  return out;
}
