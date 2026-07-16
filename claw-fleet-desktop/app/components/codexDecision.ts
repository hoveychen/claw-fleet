import type { FleetAskDecision, PendingDecision, SessionInfo } from "../types";

/**
 * Codex runs deferred MCP tools inside its outer code-mode `exec` call, so a
 * pending fleet__ask has no top-level transcript block. Select the authoritative
 * store decision for the open Codex session; other sources keep their existing
 * direct-tool rendering path.
 */
export function inlineCodexFleetAsk(
  session: SessionInfo | null,
  decisions: PendingDecision[],
): FleetAskDecision | null {
  if (!session || session.agentSource !== "codex") return null;
  return (
    decisions.find(
      (d): d is FleetAskDecision =>
        d.kind === "fleet-ask" && d.request.sessionId === session.id,
    ) ?? null
  );
}
