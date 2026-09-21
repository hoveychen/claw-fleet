// Contents of the "live status track" below the session detail page header, extracted
// as a pure function.
//
// Why we need it: the boss gave three critiques (tabs cramped in one row, not enough
// info, looks like a webpage not an app) — they're actually three symptoms of the same
// chrome problem. The old header spent 200px on "title + five-line static fields panel +
// six tabs ~46px each", while what's actually happening in this session right now —
// it's registered a watch waiting for something, it fanned out three subagents, its plan
// is at P3/5, two cards are waiting for someone to answer — none of that showed. The
// data is already on `SessionInfo` (all of it in relay's SNAPSHOT_FIELDS whitelist),
// we just needed room.
//
// This track's rule is **only show what's true right now**: no watch → no watch pill,
// no subagents → no subagent pill, empty session → track doesn't render (returns empty
// array, caller doesn't draw). This is the opposite trade-off from the old panel's
// fixed-row table ("Model —", "Workspace —"): the fixed table's width budget is stuck
// with the worst case, while a track that only draws truth shrinks to zero on a quiet
// session.
//
// Static fields (model, reasoning strength, workspace, session id) aren't here —
// they don't change, they belong on the "session detail half-screen" chip row, see
// sessionInfoRows.ts. This track is only for things that change.
//
// The desktop equivalent is the chip row in SessionDetail.tsx's header; the desktop can
// fit a full row horizontally, so it doesn't need the "only draw what's true" rule.

import { t } from "../i18n";
import type { SessionInfo, SessionStatus } from "../types";

/** Full-page content that can be pushed from the session detail page.
 *
 *  These are the five tabs from the old tab bar — their content hasn't changed at all
 *  (the five components in `SessionDetailTabs.tsx` are reused as-is), only the entry
 *  point changed: from "six cramped in one row, ~46px each" to "a full page pushed from
 *  the session detail half-screen or status pill". One pane at a time, and that pane
 *  gets the full screen width. */
export type DetailPane =
  | "decisions"
  | "plans"
  | "token"
  | "workflow"
  | "notes"
  | "handoff"
  /** Side questions asked about passages of this session's transcript. */
  | "explains";

/** Which pane clicking a pill in the track below the header pushes open.
 *
 *  `sheet` = open the "session detail" half-screen (watches and subagents don't have
 *  their own full pages, their details live in the half-screen). */
export type PillTarget = DetailPane | "sheet";

/** Three tones. `alert` is "this blocks you" (a card waiting for you, depleted budget,
 *  disconnected remote), `live` is "it's moving right now", `neutral` is background
 *  reading. Deliberately only three: more than three colors on a pill track stops being
 *  hierarchy and becomes noise. */
export type PillTone = "alert" | "live" | "neutral";

export interface StatusPill {
  /** Stable identifier. Tests assert on it (label changes with language, numeric values
   *  change with data), CSS doesn't depend on it. */
  key: string;
  label: string;
  tone: PillTone;
  /** Draw a dot that tracks the text color — only for the "it's moving right now"
   *  pill, to replace the old header's pulsing status dot in the top right. */
  dot?: boolean;
  /** Which pane clicking it pushes open; absent = read-only, not clickable. */
  target?: PillTarget;
}

/** The set of statuses for "it's moving right now". Same list as SessionDetailView's
 *  WORKING (waitingInput / active don't count — those are paused waiting for someone,
 *  not running). */
const WORKING: SessionStatus[] = ["thinking", "executing", "streaming", "processing", "delegating"];

export interface PillInput {
  /** Count of pending decision cards for this session. Not on `SessionInfo` — decision
   *  cards are a per-device aggregated inbox (App.tsx's `aggregateDecisions`), so the
   *  caller counts them by sessionId and passes them in. */
  pendingDecisions?: number;
}

/**
 * The pills to draw on this track, in fixed order.
 *
 * Order is not by importance, but by **whether it blocks you**: first the blockers
 * (depleted budget, remote disconnected, cards waiting for your answer), then what's
 * moving right now, then progress readings. The first few are what you need to handle
 * immediately, the last few are what you glance at — when scrolling horizontally,
 * the latter should slide out of view first.
 */
export function buildStatusPills(s: SessionInfo, opts: PillInput = {}): StatusPill[] {
  const pills: StatusPill[] = [];

  // ── Blockers ────────────────────────────────────────────────────────────────
  // Depleted budget has no reset moment (we're waiting for someone to recharge, not a
  // clock), so it neither changes status nor triggers auto-resume — this pill is the
  // only place on mobile that will say it.
  if (s.outOfCredits) {
    pills.push({ key: "outOfCredits", label: t("额度耗尽"), tone: "alert" });
  }
  // Remote workspace's rca-over-ssh transport is broken or Fleet killed the agent.
  // Status says remoteDisconnected but not which host or why — that detail is in the
  // half-screen.
  if (s.remoteDisconnect) {
    pills.push({ key: "remoteDisconnect", label: t("远端断开"), tone: "alert", target: "sheet" });
  }
  const pending = opts.pendingDecisions ?? 0;
  if (pending > 0) {
    pills.push({
      key: "decisions",
      label: t("{0} 张待决策", pending),
      tone: "alert",
      target: "decisions",
    });
  }

  // ── Moving right now ────────────────────────────────────────────────────────
  if (WORKING.includes(s.status)) {
    pills.push({ key: "running", label: t("运行中"), tone: "live", dot: true });
  }
  // Follow-up messages queued during a turn, sent out by `claude --resume` when the
  // turn ends. In the old UI these messages disappeared once sent, and people didn't
  // know they were still in the queue.
  if (s.pendingMessages && s.pendingMessages.length > 0) {
    pills.push({
      key: "queued",
      label: t("{0} 条排队", s.pendingMessages.length),
      tone: "live",
    });
  }
  if (s.runningSubagentCount && s.runningSubagentCount > 0) {
    pills.push({
      key: "subagents",
      label: t("{0} 个子代理", s.runningSubagentCount),
      tone: "live",
      target: "sheet",
    });
  }
  if (s.watches && s.watches.length > 0) {
    // A watch whose until command cannot run at all is not waiting, it is
    // stuck — the poll count is a reassuring lie in that state, so show an
    // alert pill instead of it.
    const broken = s.watches.filter((w) => (w.structuralFailStreak ?? 0) > 0).length;
    if (broken > 0) {
      pills.push({
        key: "watch",
        label: t("watch 跑不起来 ×{0}", broken),
        tone: "alert",
        target: "sheet",
      });
    } else {
      // For one watch, report how many times it has polled — that's the only visible
      // evidence that "it's alive and still waiting". For multiple, report the count;
      // each one's poll count is in the half-screen.
      const label =
        s.watches.length === 1
          ? t("watch ×{0}", s.watches[0].pollCount)
          : t("{0} 个 watch", s.watches.length);
      pills.push({ key: "watch", label, tone: "live", target: "sheet" });
    }
  }

  // ── Progress readings ───────────────────────────────────────────────────────
  if (s.taskPlan && s.taskPlan.total > 0) {
    pills.push({
      key: "plan",
      label: t("计划 {0}/{1}", s.taskPlan.done, s.taskPlan.total),
      tone: "neutral",
      target: "plans",
    });
  }
  if (s.handoff) {
    pills.push({
      key: "handoff",
      label: t("接力 {0}/{1}", s.handoff.hop, s.handoff.chainLen),
      tone: "neutral",
      target: "handoff",
    });
  }
  // contextPercent is a 0–1 ratio (aligned with desktop SessionDetail's `* 100` usage).
  if (s.contextPercent != null) {
    pills.push({
      key: "context",
      label: t("上下文 {0}%", Math.round(s.contextPercent * 100)),
      tone: "neutral",
      target: "token",
    });
  }
  // Half a cent or less showing as $0.00 is the same as saying nothing; same threshold
  // as desktop and sessionInfoRows.
  if (s.totalCostUsd != null && s.totalCostUsd >= 0.005) {
    pills.push({
      key: "cost",
      label: `$${s.totalCostUsd.toFixed(2)}`,
      tone: "neutral",
      target: "token",
    });
  }

  return pills;
}
