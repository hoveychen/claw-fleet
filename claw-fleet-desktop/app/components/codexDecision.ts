import type {
  DecisionHistoryRecord,
  FleetAskDecision,
  PendingDecision,
  RawMessage,
  SessionInfo,
} from "../types";

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

function messageToolUseIds(messages: RawMessage[]): Set<string> {
  const ids = new Set<string>();
  for (const message of messages) {
    const content = message.message?.content;
    if (!Array.isArray(content)) continue;
    for (const block of content) {
      if (block.type === "tool_use" && typeof block.id === "string") ids.add(block.id);
    }
  }
  return ids;
}

function timestampMillis(value: string | undefined): number | null {
  if (!value) return null;
  const millis = Date.parse(value);
  return Number.isFinite(millis) ? millis : null;
}

function historyMessage(
  record: Extract<DecisionHistoryRecord, { kind: "fleet-ask" }>,
): RawMessage {
  return {
    type: "assistant",
    uuid: `codex-decision-history-${record.id}`,
    timestamp: record.requestedAt,
    isVisibleInTranscriptOnly: true,
    message: {
      role: "assistant",
      stop_reason: "end_turn",
      content: [{
        type: "tool_use",
        id: record.id,
        name: "mcp__fleet__fleet__ask",
        input: { questions: record.questions },
      }],
    },
  };
}

/**
 * Codex's nested `fleet__ask` call is absent from its rollout transcript. Once
 * the pending-store copy resolves, reconstruct one ordinary decision-tool row
 * from durable history and merge it at the request time. Real transcript rows
 * retain their original relative order; a future Codex format that exposes the
 * tool call directly is protected by the tool-use id de-duplication.
 */
export function withCodexDecisionHistory(
  session: SessionInfo | null,
  messages: RawMessage[],
  records: DecisionHistoryRecord[],
): RawMessage[] {
  if (!session || session.agentSource !== "codex") return messages;

  const existingIds = messageToolUseIds(messages);
  const history = records
    .filter(
      (record): record is Extract<DecisionHistoryRecord, { kind: "fleet-ask" }> =>
        record.kind === "fleet-ask"
        && record.sessionId === session.id
        && record.questions.length > 0
        && !existingIds.has(record.id),
    )
    .map((record, order) => ({ record, order, at: timestampMillis(record.requestedAt) }))
    .sort((a, b) => {
      if (a.at === null && b.at === null) return a.order - b.order;
      if (a.at === null) return 1;
      if (b.at === null) return -1;
      return a.at - b.at || a.order - b.order;
    });

  if (history.length === 0) return messages;

  const merged: RawMessage[] = [];
  let historyIndex = 0;
  for (const message of messages) {
    const messageAt = timestampMillis(message.timestamp);
    while (
      messageAt !== null
      && historyIndex < history.length
      && history[historyIndex].at !== null
      && history[historyIndex].at! <= messageAt
    ) {
      merged.push(historyMessage(history[historyIndex].record));
      historyIndex += 1;
    }
    merged.push(message);
  }
  while (historyIndex < history.length) {
    merged.push(historyMessage(history[historyIndex].record));
    historyIndex += 1;
  }
  return merged;
}
