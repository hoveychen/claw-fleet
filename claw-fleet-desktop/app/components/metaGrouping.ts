import type { RawMessage } from "../types";
import { dayKey } from "../messageRows";

/**
 * Rows that `MessageRow` folds into a `MetaFoldBlock` — synthetic harness/runtime
 * user turns (a `Skill` load's SKILL.md body, codex's developer-role
 * sandbox/permissions/collaboration/`<multi_agent_mode>` preamble). Mirrors the
 * branch order in `MessageRow`: a compact-summary turn is its own banner and must
 * never be swept into a meta group.
 */
export function isMetaRow(msg: RawMessage): boolean {
  return msg.type === "user" && !!msg.isMeta && !msg.isCompactSummary;
}

/**
 * A run of adjacent meta rows folded into one card, or a single passthrough row.
 * `startLocal` is the index into the input array of the unit's first row, so the
 * caller can recover each row's global message index and day-separator state.
 */
export type MetaRenderUnit =
  | { kind: "meta-group"; startLocal: number; msgs: RawMessage[] }
  | { kind: "single"; startLocal: number; msg: RawMessage };

/**
 * Collapse runs of adjacent `isMeta` user turns into single `meta-group` units,
 * leaving every other row as a `single`. N back-to-back injected turns otherwise
 * render as N identical "系统上下文" dividers.
 *
 * A group needs at least two rows — a lone meta row stays a `single` and renders
 * exactly as before. Runs break at day boundaries so the day-separator invariant
 * (one separator per day, on that day's first row) still holds after grouping.
 */
export function groupMetaRuns(visibleMsgs: RawMessage[]): MetaRenderUnit[] {
  const units: MetaRenderUnit[] = [];
  for (let k = 0; k < visibleMsgs.length; ) {
    const msg = visibleMsgs[k];
    if (isMetaRow(msg)) {
      const day = dayKey(msg.timestamp);
      let j = k + 1;
      while (
        j < visibleMsgs.length &&
        isMetaRow(visibleMsgs[j]) &&
        dayKey(visibleMsgs[j].timestamp) === day
      ) {
        j++;
      }
      if (j - k >= 2) {
        units.push({ kind: "meta-group", startLocal: k, msgs: visibleMsgs.slice(k, j) });
        k = j;
        continue;
      }
    }
    units.push({ kind: "single", startLocal: k, msg });
    k++;
  }
  return units;
}
