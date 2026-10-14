/**
 * Session state → what its lobster does. Pure, so the mapping is tested
 * without a canvas. Scarf colours follow rezored/cubicle-crew's state palette.
 */

import type { PendingDecisionState } from "../../pendingDecisionState";
import type { SessionInfo } from "../../types";
import type { Pose } from "./sprites";

export const STATE_COLORS = {
  idle: "#6b6b80",
  thinking: "#e0c24a",
  typing: "#4ac26b",
  reading: "#4a8fe0",
  running: "#e07a4a",
  delegating: "#b04ae0",
  waiting: "#e04a6b",
} as const;

export interface Look {
  pose: Pose;
  color: string;
  /** A card (or a native prompt) is waiting on the user. `parked` = the card
   *  timed out and the turn stopped on it. */
  alert: PendingDecisionState;
}

const WRITE_TOOLS = new Set(["Edit", "Write", "MultiEdit", "NotebookEdit"]);
const READ_TOOLS = new Set(["Read", "Grep", "Glob", "LS", "WebSearch", "WebFetch", "ToolSearch"]);
const AGENT_TOOLS = new Set(["Task", "Agent"]);

/** Claude Code's own prompt, or Fleet's decision card under any MCP prefix. */
export function isAskTool(tool: string): boolean {
  return tool === "AskUserQuestion" || tool === "ExitPlanMode" || /(^|__)fleet__ask$/.test(tool);
}

/** The pose a tool call reads as. Unknown tools (MCP and the like) are "work". */
export function toolLook(tool: string): Pick<Look, "pose" | "color"> {
  if (isAskTool(tool)) return { pose: "wave", color: STATE_COLORS.waiting };
  if (WRITE_TOOLS.has(tool)) return { pose: "type", color: STATE_COLORS.typing };
  if (READ_TOOLS.has(tool)) return { pose: "read", color: STATE_COLORS.reading };
  if (AGENT_TOOLS.has(tool)) return { pose: "delegate", color: STATE_COLORS.delegating };
  return { pose: "run", color: STATE_COLORS.running };
}

export function lookFor(s: SessionInfo, decision: PendingDecisionState = "none"): Look {
  if (decision !== "none") return { pose: "wave", color: STATE_COLORS.waiting, alert: decision };
  const tool = s.currentTool ?? null;
  if (tool && isAskTool(tool)) return { ...toolLook(tool), alert: "pending" };
  switch (s.status) {
    case "rateLimited":
    case "serverErrored":
    case "remoteDisconnected":
      return { pose: "error", color: STATE_COLORS.waiting, alert: "none" };
    case "stuck":
      return { pose: "stuck", color: STATE_COLORS.waiting, alert: "none" };
    case "watching":
      return { pose: "watch", color: STATE_COLORS.reading, alert: "none" };
    case "delegating":
      return { pose: "delegate", color: STATE_COLORS.delegating, alert: "none" };
    case "executing":
      return { ...(tool ? toolLook(tool) : { pose: "run" as Pose, color: STATE_COLORS.running }), alert: "none" };
    case "streaming":
      return { pose: "type", color: STATE_COLORS.typing, alert: "none" };
    case "thinking":
    case "processing":
    case "active":
      return { ...(tool ? toolLook(tool) : { pose: "think" as Pose, color: STATE_COLORS.thinking }), alert: "none" };
    case "waitingInput":
      // For a main session this is almost always "turn ended" — a real
      // question to the user arrives as a card or an ask tool, handled above.
      return { pose: "coffee", color: STATE_COLORS.idle, alert: "none" };
    case "idle":
      return { pose: s.procAlive ? "coffee" : "idle", color: STATE_COLORS.idle, alert: "none" };
  }
}

/** Poses that move fast enough to need the full frame rate. */
export const BUSY_POSES = new Set<Pose>(["type", "run", "delegate", "wave", "error", "think"]);
