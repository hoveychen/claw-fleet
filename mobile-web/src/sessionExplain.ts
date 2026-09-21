// Side questions about a session's transcript ("selection explain"), phone
// side: the three relay methods behind one typed surface.
//
// The relay arms mirror the desktop's Tauri commands one for one
// (`claw-fleet-core/src/mobile_relay.rs`: session_explain_ask / session_explain /
// session_explain_list). `ask` forks the session and spends money, so it rides
// the relay's idempotent-write path: a reply lost on a flaky link is retried
// with the same key and the host replays the first record instead of forking
// twice. The poll loop that turns the record file into a stream is shared
// with the desktop (`shared-ts/sessionExplain.ts`).

import type { ExplainRecord, ExplainRequest } from "./generated/types";
import type { FleetTransport } from "./transport";

export type { ExplainAnchor, ExplainPreset, ExplainRecord, ExplainRequest, ExplainStatus } from "./generated/types";

function freshIdempotencyKey(): string {
  return globalThis.crypto?.randomUUID?.() ?? `explain-${Date.now()}-${Math.random()}`;
}

/** Accept a side question; resolves with the `running` record right away. */
export function askExplanation(
  client: FleetTransport,
  request: ExplainRequest,
  idempotencyKey: string = freshIdempotencyKey(),
): Promise<ExplainRecord> {
  return client.request<ExplainRecord>("session_explain_ask", { ...request, idempotencyKey });
}

/** One record as it stands on the host. Rejects when the id is unknown. */
export function getExplanation(client: FleetTransport, sessionId: string, id: string): Promise<ExplainRecord> {
  return client.request<ExplainRecord>("session_explain", { sessionId, id });
}

/** Every record of the session, oldest first. */
export function listExplanations(client: FleetTransport, sessionId: string): Promise<ExplainRecord[]> {
  return client.request<ExplainRecord[]>("session_explain_list", { sessionId });
}

/**
 * The failed-card stand-in for an ask the host refused (no source owns the
 * session, the fork would not spawn, the link dropped). Nothing was stored, so
 * the record is local to the view and carries the message where the answer
 * would have been — the same shape the desktop builds.
 */
export function refusedExplanation(req: ExplainRequest, error: unknown): ExplainRecord {
  const now = Date.now();
  return {
    id: `local-${now}`,
    sessionId: req.sessionId,
    source: "",
    createdMs: now,
    updatedMs: now,
    preset: req.preset,
    quote: req.quote,
    question: req.question ?? "",
    anchor: req.anchor,
    thread: req.thread ?? [],
    status: "error",
    text: "",
    error: error instanceof Error ? error.message : String(error),
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    durationMs: 0,
    // Never reached the store, so there is no dismissal to read back.
    dismissed: false,
  };
}
