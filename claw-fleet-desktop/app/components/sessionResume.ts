/**
 * Resuming a stopped session — the one command and the one eligibility gate
 * behind every "continue this" control in the app.
 *
 * Four controls on `SessionCard` (rate limit, server error, remote disconnect,
 * out of credits) had each grown their own verbatim copy of the same three
 * lines, and the failed-turn card in the transcript would have been a fifth.
 * The gate is not arbitrary trivia — it mirrors `auto_resume.rs`'s
 * `should_auto_resume` — so five copies is five places for it to drift away
 * from the scheduler that actually does the resuming.
 */
import { invoke } from "@tauri-apps/api/core";

import type { SessionInfo } from "../types";

/** Sources whose sessions can be resumed headlessly: `claude --resume` and
 *  `codex exec resume`. dsh and imported transcripts cannot. */
export const RESUMABLE_SOURCES = ["claude-code", "codex"];

/** Enough of a session to decide whether a resume is even offerable. Kept
 *  structural so callers holding a partial session (or a card context) can ask
 *  without materialising a whole `SessionInfo`. */
export type ResumableSession = Pick<SessionInfo, "isSubagent" | "ideName" | "agentSource">;

/**
 * Whether a resume control should be shown at all.
 *
 * Mirrors the auto-resume scheduler's gate:
 *  - a subagent's `agent-*` transcript cannot be resumed;
 *  - a session attached to an interactive IDE (VS Code, the Claude app) should
 *    be resumed from the editor — firing a detached headless resume behind it
 *    puts two agents on one transcript;
 *  - the source has to support headless resume.
 */
export function canResumeSession(session: ResumableSession): boolean {
  return (
    !session.isSubagent && !session.ideName && RESUMABLE_SOURCES.includes(session.agentSource)
  );
}

/**
 * A resume that fails never starts a process, so nothing downstream will ever
 * show the user why — the control that fired it is the only surface left. Tauri
 * rejects a command with the plain string the Rust side returned (e.g.
 * "Workspace directory not found: …"); anything else gets stringified as-is.
 */
export function resumeErrorText(err: unknown): string {
  if (typeof err === "string") return err;
  const msg = (err as { message?: unknown } | null)?.message;
  return typeof msg === "string" ? msg : String(err);
}

export interface ResumeArgs {
  sessionId: string;
  workspacePath: string;
  agentSource: string;
  /** Sent as the resumed turn's prompt. Omitted, the agent re-runs the turn the
   *  failure interrupted, which is what every "retry" control wants. */
  prompt?: string;
  /** Overrides the model the session launched with — the escape from a
   *  per-model quota. Omitted, the session keeps its own. */
  model?: string;
}

/** Fire the resume. Rejects with the backend's message; callers surface it via
 *  [`resumeErrorText`]. */
export async function resumeSession({
  sessionId,
  workspacePath,
  agentSource,
  prompt,
  model,
}: ResumeArgs): Promise<void> {
  await invoke("resume_rate_limited_session", {
    sessionId,
    workspacePath,
    agentSource,
    ...(prompt ? { prompt } : {}),
    ...(model ? { model } : {}),
  });
}
