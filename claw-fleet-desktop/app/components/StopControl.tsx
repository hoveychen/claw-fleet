import { useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { ask } from "@tauri-apps/plugin-dialog";
import type { SessionInfo, SessionStatus } from "../types";
import { isFleetOwnedEntrypoint } from "../types";
import styles from "./StopControl.module.css";

/** The agent is mid-turn: thinking, emitting a tool call, waiting on a tool
 *  result, or driving a subagent. Only then is there something to interrupt. */
const TURN_IN_FLIGHT = new Set<SessionStatus>([
  "thinking",
  "executing",
  "streaming",
  "processing",
  "delegating",
  // A wedged tool-use batch: the turn is in flight but deadlocked. SIGINT is
  // exactly the fix — it completes the batch with an interrupt result and
  // leaves the transcript resumable (same mechanics as any interrupt).
  "stuck",
]);

export type StopMode = "interrupt" | "stop" | "spent";

/**
 * One button, three states, escalating with each click:
 *
 *   interrupt → the turn is in flight; SIGINT aborts the tool call and leaves
 *               the transcript resumable
 *   stop      → nothing to interrupt (or nothing safe to); kill the tree
 *   spent     → no live process left; the button is dead
 *
 * `interrupt` is deliberately narrow. SIGINT only means "abort the tool call"
 * for the headless `-p` sessions Fleet spawns itself; an interactive claude in
 * a terminal reads Ctrl-C as a keystroke, so a real SIGINT makes it *quit* and
 * abandon its tool child. Offering "interrupt" there would be a hard kill
 * wearing a friendly label, so those sessions go straight to `stop`. It also
 * needs an unambiguous pid: with several claude processes sharing a cwd we
 * would be aborting whichever turn we guessed at, and unlike `stop` there is no
 * confirmation dialog to catch the mistake.
 */
/**
 * Sources with no per-session process, whose stop must go through the source
 * rather than a signal.
 *
 * dsh is the case: every dsh session's turn runs inside one shared `dsh web`,
 * so the pid on its `SessionInfo` is that *server's*. Signalling it would stop
 * every dsh session on the machine and take Fleet's own server down with them —
 * and the `!pidPrecise` fallback below is no better, since it kills by
 * workspace. `session.cancel` is the per-session lever, reached through
 * `interrupt_agent_session`.
 */
export function usesSourceInterrupt(s: SessionInfo): boolean {
  return s.agentSource === "dsh";
}

export function stopMode(s: SessionInfo): StopMode {
  if (s.pid === null) return "spent";
  // The source-level stop only ever cancels a turn, so there is nothing to
  // offer once the session is idle — and nothing that could be a hard kill.
  if (usesSourceInterrupt(s)) {
    return TURN_IN_FLIGHT.has(s.status) ? "interrupt" : "spent";
  }
  if (
    isFleetOwnedEntrypoint(s.entrypoint) &&
    s.pidPrecise &&
    TURN_IN_FLIGHT.has(s.status)
  ) {
    return "interrupt";
  }
  return "stop";
}

/** Subagents are driven by their parent, so they are not ours to signal. */
export function canControl(s: SessionInfo): boolean {
  return !s.isSubagent;
}

/**
 * Perform the escalating stop action for a session — the same behaviour the
 * StopControl button carries, factored out so the row's context menu can drive
 * it too (the confirmation dialogs and the interrupt-vs-kill choice must stay
 * identical in both places). No-op when there is nothing live to signal.
 */
export async function performStop(
  session: SessionInfo,
  t: (key: string, opts?: Record<string, unknown>) => string,
): Promise<void> {
  const mode = stopMode(session);
  if (mode === "spent") return;
  // Routed by session, never by pid — see `usesSourceInterrupt`. No confirmation
  // dialog: this cancels the current turn and nothing else, and the session
  // stays exactly as resumable as an agent that finished on its own.
  if (usesSourceInterrupt(session)) {
    await invoke("interrupt_agent_session", { path: session.jsonlPath });
    return;
  }
  if (mode === "interrupt") {
    await invoke("interrupt_session", { pid: session.pid });
  } else if (session.pidPrecise) {
    const ok = await ask(t("stop_confirm", { name: session.workspaceName }), { kind: "warning" });
    if (ok) await invoke("kill_session", { pid: session.pid });
  } else {
    const ok = await ask(t("stop_imprecise_confirm", { workspace: session.workspaceName }), { kind: "warning" });
    if (ok) await invoke("kill_workspace_sessions", { workspacePath: session.workspacePath });
  }
}

export function StopControl({
  session,
  variant = "bare",
}: {
  session: SessionInfo;
  /**
   * `"bare"` (default): the small ghost glyph used in session lists/cards.
   * `"solid"`: a filled round button sized to sit in the composer's send slot,
   * so a running-session Stop reads as the primary action there.
   */
  variant?: "bare" | "solid";
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const mode = stopMode(session);

  const onClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (busy || mode === "spent") return;
    setBusy(true);
    try {
      await performStop(session, t);
    } finally {
      setBusy(false);
    }
  };

  const title =
    mode === "interrupt"
      ? session.status === "stuck"
        ? t("unstick_session")
        : t("interrupt_session")
      : mode === "spent"
        ? t("stop_spent")
        : session.pidPrecise
          ? t("stop_session")
          : t("stop_session_imprecise");

  return (
    <button
      type="button"
      className={[
        styles.btn,
        variant === "solid" ? styles.btn_solid : "",
        styles[`btn_${mode}`],
        busy ? styles.btn_busy : "",
        mode === "stop" && !session.pidPrecise ? styles.btn_warn : "",
      ]
        .filter(Boolean)
        .join(" ")}
      onClick={onClick}
      title={title}
      disabled={busy || mode === "spent"}
      aria-label={title}
    >
      {mode === "interrupt" ? "⎋" : "■"}
    </button>
  );
}
