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
export function stopMode(s: SessionInfo): StopMode {
  if (s.pid === null) return "spent";
  if (
    isFleetOwnedEntrypoint(s.entrypoint) &&
    s.pidPrecise &&
    TURN_IN_FLIGHT.has(s.status)
  ) {
    return "interrupt";
  }
  return "stop";
}

/** Subagents are driven by their parent; cursor sessions are not ours to signal. */
export function canControl(s: SessionInfo): boolean {
  return !s.isSubagent && s.agentSource !== "cursor";
}

export function StopControl({ session }: { session: SessionInfo }) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const mode = stopMode(session);

  const onClick = async (e: React.MouseEvent) => {
    e.stopPropagation();
    if (busy || mode === "spent") return;
    setBusy(true);
    try {
      if (mode === "interrupt") {
        await invoke("interrupt_session", { pid: session.pid });
      } else if (session.pidPrecise) {
        const ok = await ask(t("stop_confirm", { name: session.workspaceName }), { kind: "warning" });
        if (ok) await invoke("kill_session", { pid: session.pid });
      } else {
        const ok = await ask(t("stop_imprecise_confirm", { workspace: session.workspaceName }), { kind: "warning" });
        if (ok) await invoke("kill_workspace_sessions", { workspacePath: session.workspacePath });
      }
    } finally {
      setBusy(false);
    }
  };

  const title =
    mode === "interrupt"
      ? t("interrupt_session")
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
