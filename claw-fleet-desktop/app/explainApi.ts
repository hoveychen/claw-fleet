/**
 * Side questions about a session's transcript ("selection explain").
 *
 * Thin typed wrappers over the three host commands plus the poll loop every
 * surface shares: the host answers in a fork on its own thread and rewrites the
 * record file as text streams in, so "streaming" on the client is nothing more
 * than re-reading the record until its status leaves `running`. See
 * `claw-fleet-core/src/session_explain.rs` for the record's lifecycle.
 */
import { invoke } from "@tauri-apps/api/core";

import type { ExplainRecord, ExplainRequest } from "./generated/types";

export type { ExplainAnchor, ExplainPreset, ExplainRecord, ExplainRequest, ExplainStatus } from "./generated/types";

/** Accept a side question; resolves with the `running` record right away. */
export function explainSelection(request: ExplainRequest): Promise<ExplainRecord> {
  return invoke<ExplainRecord>("explain_selection", { request });
}

/** One record as it stands on disk. Rejects when the id is unknown. */
export function getExplanation(sessionId: string, id: string): Promise<ExplainRecord> {
  return invoke<ExplainRecord>("get_explanation", { sessionId, id });
}

/** Every record of the session, oldest first. */
export function listExplanations(sessionId: string): Promise<ExplainRecord[]> {
  return invoke<ExplainRecord[]>("list_explanations", { sessionId });
}

/** How often a running record is re-read. The host flushes every ~120 ms. */
export const EXPLAIN_POLL_MS = 300;

/**
 * Re-read a record until it settles, reporting every observed change.
 *
 * `fetch` is injected so the loop is testable and reusable by a transport
 * other than Tauri `invoke` (the mobile relay has its own). A fetch failure is
 * treated as "not yet readable" and retried: the record is written atomically,
 * but the first read can race the worker's first flush. `signal` aborts the
 * loop; the promise then resolves with the last record seen (or `null`).
 */
export async function pollExplanation(
  fetch: () => Promise<ExplainRecord>,
  onUpdate: (rec: ExplainRecord) => void,
  opts: { intervalMs?: number; signal?: AbortSignal; sleep?: (ms: number) => Promise<void> } = {},
): Promise<ExplainRecord | null> {
  const interval = opts.intervalMs ?? EXPLAIN_POLL_MS;
  const sleep = opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  let last: ExplainRecord | null = null;
  let lastKey = "";
  while (!opts.signal?.aborted) {
    let rec: ExplainRecord | null = null;
    try {
      rec = await fetch();
    } catch {
      rec = null;
    }
    if (rec) {
      const key = `${rec.status}:${rec.updatedMs}:${rec.text.length}`;
      if (key !== lastKey) {
        lastKey = key;
        last = rec;
        onUpdate(rec);
      }
      if (rec.status !== "running") return rec;
    }
    await sleep(interval);
  }
  return last;
}
