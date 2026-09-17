// Content of the chip row at the top of the session detail half-sheet, extracted into a pure function.
//
// Its predecessor was an inline-expanded panel below the header: a five-row label/value table
// consuming ~90px to display five short terms. The table alignment there provided no benefit — these
// fields are all "at-a-glance" readings (correct model?, which workspace?, how much spent?), needing
// no vertical column alignment. After switching to a single line of wrappable chips, the same five
// items occupy ~26px, freeing up height for what the half-sheet really needs to unfold: what watch is
// waiting for, how many sub-agents are running, where the plan is at.
//
// The construction logic is here rather than in the component so it can be pinned down by unit tests:
// "which fields disappear when absent" — a chip saying "model —" is worse than no chip.
//
// Only these items deliberately: model, inference strength, workspace, context usage, cost. Paths are
// not here — they sit as-is in the copy row and secondary row of the "Session" section on the
// half-sheet, which is where paths are actually used (copied); timestamps are not here either, they're
// beside each message, and the session list has "a few minutes ago". State, watch, sub-agents, plan,
// handoff all change — they're in the status track and the half-sheet's "Now" / "Progress" sections;
// see sessionStatusPills.ts.
//
// The desktop equivalent is `meta_row` in SessionDetail.tsx (with more fields there, since desktop can
// fit a full row of chips horizontally).

import { t } from "../i18n";
import { toolForAgentSource } from "../agentSource";
import type { SessionInfo } from "../types";

/**
 * Copy for the chip row at the top of the half-sheet, in fixed order. Absent fields produce no chip —
 * produce nothing rather than an empty one — a session with no recorded model has one fewer chip, not
 * one more "model —".
 *
 * The first three are bare values: model name, inference strength tier, workspace name are
 * self-explanatory; adding a label would be redundant. The last two carry labels: a lone "40%" doesn't
 * clarify whether it's context or something else.
 */
export function buildInfoChips(s: SessionInfo): string[] {
  const chips: string[] = [];
  const push = (v: string | undefined | null) => {
    const trimmed = (v ?? "").trim();
    if (trimmed) chips.push(trimmed);
  };

  push(s.model);
  // Right after model: these two together explain "what compute this session is using". On desktop
  // header, they're also adjacent chips.
  push(s.effort);
  push(s.workspaceName);
  // contextPercent is a 0–1 ratio (aligning with desktop SessionDetail's `* 100` usage), not a percentage.
  if (s.contextPercent != null) {
    chips.push(t("上下文 {0}%", Math.round(s.contextPercent * 100)));
  }
  // Costs below half a cent display as $0.00, same as not mentioning it; same threshold as desktop.
  if (s.totalCostUsd != null && s.totalCostUsd >= 0.005) {
    chips.push(`$${s.totalCostUsd.toFixed(2)}`);
  }
  return chips;
}

/**
 * Command that can be pasted directly into a terminal to resume this session.
 *
 * Only for Claude source: `claude --resume <id>` (see comment at top of `claw-fleet-core/session_launch.rs`).
 * For Codex, resume is `codex exec resume <id>`, which is a headless form, interactive form differs; rather
 * than provide a command that might error on paste, this item simply doesn't appear (returns null, menu doesn't
 * render it accordingly). Dsh has no CLI resume entry.
 */
export function resumeCommand(s: SessionInfo): string | null {
  return toolForAgentSource(s.agentSource) === "claude" ? `claude --resume ${s.id}` : null;
}
