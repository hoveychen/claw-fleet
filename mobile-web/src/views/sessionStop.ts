// All the logic and execution for the "stop" action — shared by the task list card and session detail half-screen.
//
// Previously it only existed on the TasksView card (`stopMode` + `handleStop` as local definitions),
// so when viewing an active session on the session detail page, the only way to stop it was to go back
// to the list and find that card. After extracting here, both places use the same three-state logic and
// the same confirmation message, so they don't drift apart.
//
// pid / workspacePath only has meaning on their own host: the caller must pass the transport
// for that device. Sending to a different device means either it won't stop, or worse, the pid
// is matched against an unrelated process.

import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { SessionInfo, SessionStatus } from "../types";
import { isFleetOwnedEntrypoint } from "../types";

const WORKING: SessionStatus[] = [
  "thinking",
  "executing",
  "streaming",
  "processing",
  "delegating",
];

export type StopMode = "interrupt" | "stop" | "spent";

/** dsh runs every session inside one shared `dsh web`, so there is no
 *  per-session process to signal: its stop is a turn cancel through the source,
 *  addressed by session uri. Mirrors the desktop's `usesSourceInterrupt`. */
export function usesSourceInterrupt(s: SessionInfo): boolean {
  return s.agentSource === "dsh";
}

/** Same escalation as the desktop StopControl: interrupt only for Fleet-owned
 *  sessions with a precise pid mid-turn; otherwise kill; no pid → dead. */
export function stopMode(s: SessionInfo): StopMode {
  // Before the pid test: a dsh row has no pid, and only a turn to cancel.
  if (usesSourceInterrupt(s)) return WORKING.includes(s.status) ? "interrupt" : "spent";
  if (s.pid == null) return "spent";
  if (isFleetOwnedEntrypoint(s.entrypoint) && s.pidPrecise && WORKING.includes(s.status)) {
    return "interrupt";
  }
  return "stop";
}

/** Subagents have no process of their own and cannot be stopped; the button should not be shown. */
export function canControl(s: SessionInfo): boolean {
  return !s.isSubagent;
}

/** Execute a stop/interrupt.
 *
 *  Returns `false` if the user cancelled the confirmation dialog (or the session had nothing to stop);
 *  the caller uses this to clear the busy state. Throwing an error means the relay request truly failed;
 *  the caller decides how to display the error. */
export async function runStop(
  transport: FleetTransport,
  s: SessionInfo,
  confirm: (msg: string) => Promise<boolean>,
): Promise<boolean> {
  const mode = stopMode(s);
  if (mode === "spent") return false;
  if (usesSourceInterrupt(s)) {
    await transport.request("interrupt", { path: s.jsonlPath });
    return true;
  }
  if (mode === "interrupt") {
    await transport.request("interrupt", { pid: s.pid });
    return true;
  }
  if (!(await confirm(t("确定停止「{0}」这个任务吗？", s.workspaceName)))) return false;
  await transport.request("stop", { pid: s.pid });
  return true;
}
