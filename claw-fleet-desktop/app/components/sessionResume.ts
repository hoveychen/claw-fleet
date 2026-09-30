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
import i18n from "i18next";

import { retiredSuccessor } from "../../../shared-ts/retiredSession";

import type { SessionInfo } from "../types";

/** Sources whose sessions can be resumed headlessly: `claude --resume` and
 *  `codex exec resume`. dsh and imported transcripts cannot. */
export const RESUMABLE_SOURCES = ["claude-code", "codex"];

/** Enough of a session to decide whether a resume is even offerable. Kept
 *  structural so callers holding a partial session (or a card context) can ask
 *  without materialising a whole `SessionInfo`. */
export type ResumableSession = Pick<SessionInfo, "isSubagent" | "agentSource">;

/**
 * Whether a resume control should be shown at all.
 *
 * Mirrors the auto-resume scheduler's gate:
 *  - a subagent's `agent-*` transcript cannot be resumed;
 *  - the source has to support headless resume.
 */
export function canResumeSession(session: ResumableSession): boolean {
  return (
    !session.isSubagent && RESUMABLE_SOURCES.includes(session.agentSource)
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

/** Ask whether to resume a session that `successor` has taken over. */
function confirmRetired(successor: string): boolean {
  return window.confirm(
    i18n.t(
      "session.retired_resume_confirm",
      "这个会话已被 {{id}} 接替。继续它会让两个会话同时做同一份工作。仍要继续？",
      { id: successor.slice(0, 8) },
    ),
  );
}

/** Fire the resume. Rejects with the backend's message; callers surface it via
 *  [`resumeErrorText`]. A session another one has taken over asks the boss
 *  first, and rejects with a "not resumed" message when they decline. */
export async function resumeSession(
  { sessionId, workspacePath, agentSource, prompt, model }: ResumeArgs,
  confirm: (successor: string) => boolean = confirmRetired,
): Promise<void> {
  const args = {
    sessionId,
    workspacePath,
    agentSource,
    ...(prompt ? { prompt } : {}),
    ...(model ? { model } : {}),
  };
  try {
    await invoke("resume_rate_limited_session", args);
  } catch (err) {
    const successor = retiredSuccessor(err);
    if (!successor) throw err;
    if (!confirm(successor)) {
      const id = successor.slice(0, 8);
      // `||`: i18next answers undefined until it is initialised.
      throw i18n.t("session.retired_resume_declined", "未继续：已被 {{id}} 接替", { id }) || `未继续：已被 ${id} 接替`;
    }
    await invoke("resume_rate_limited_session", { ...args, allowRetired: true });
  }
}
