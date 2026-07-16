import type { RawMessage } from "../types";

/**
 * Rows that fold into a `MetaFoldCard` — synthetic harness/runtime user turns
 * (a `Skill` load's SKILL.md body, codex's developer-role
 * sandbox/permissions/collaboration/`<multi_agent_mode>` preamble). A
 * compact-summary turn is its own banner and must never be swept into a group.
 *
 * Mirrors the desktop `metaGrouping.ts`; kept as a separate copy because
 * mobile-web is a standalone package that shares no code with the desktop app.
 */
export function isMetaRow(msg: RawMessage): boolean {
  return msg.type === "user" && !!msg.isMeta && !msg.isCompactSummary;
}

/**
 * A run of adjacent meta rows folded into one card, or a single passthrough row.
 * `startLocal` is the index into the input array of the unit's first row.
 */
export type MetaRenderUnit =
  | { kind: "meta-group"; startLocal: number; msgs: RawMessage[] }
  | { kind: "single"; startLocal: number; msg: RawMessage };

/**
 * Collapse runs of adjacent `isMeta` user turns into single `meta-group` units,
 * leaving every other row a `single`. N back-to-back injected turns otherwise
 * stack as N identical "系统上下文" cards. A group needs at least two rows — a lone
 * meta row stays a `single` and renders exactly as before.
 *
 * Unlike desktop this does not break runs at day boundaries: mobile-web has no
 * day separators, so there is no invariant to protect and adjacency alone drives
 * the grouping.
 */
export function groupMetaRuns(rows: RawMessage[]): MetaRenderUnit[] {
  const units: MetaRenderUnit[] = [];
  for (let k = 0; k < rows.length; ) {
    if (isMetaRow(rows[k])) {
      let j = k + 1;
      while (j < rows.length && isMetaRow(rows[j])) j++;
      if (j - k >= 2) {
        units.push({ kind: "meta-group", startLocal: k, msgs: rows.slice(k, j) });
        k = j;
        continue;
      }
    }
    units.push({ kind: "single", startLocal: k, msg: rows[k] });
    k++;
  }
  return units;
}
