/**
 * Side questions about a session's transcript ("selection explain").
 *
 * Thin typed wrappers over the three host commands. The poll loop every
 * surface shares lives in `shared-ts/sessionExplain.ts` (the phone runs the
 * same loop over the relay) and is re-exported here so the hooks keep one
 * import. See `claw-fleet-core/src/session_explain.rs` for the record's
 * lifecycle.
 */
import { invoke } from "@tauri-apps/api/core";

import type { ExplainRecord, ExplainRequest } from "./generated/types";

export type { ExplainAnchor, ExplainPreset, ExplainRecord, ExplainRequest, ExplainStatus } from "./generated/types";
export { EXPLAIN_POLL_MS, pollExplanation } from "../../shared-ts/sessionExplain";

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

/** Take a card out of the rail, or hand it back. Persisted, so the ✕ survives
 *  a session switch and an app restart. */
export function dismissExplanation(
  sessionId: string,
  id: string,
  dismissed: boolean,
): Promise<void> {
  return invoke<void>("dismiss_explanation", { sessionId, id, dismissed });
}
