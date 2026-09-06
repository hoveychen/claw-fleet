import type { ProcRecord } from "../types";
import { isDefaultShellCommand } from "./procCommandLabel";

/** TerminalView owns interactive shells, while ProcPanel owns command history. */
export function terminalProcsForWorkspace(
  procs: ProcRecord[],
  workspacePath: string | null,
): ProcRecord[] {
  if (!workspacePath) return [];
  return procs.filter(
    (proc) =>
      proc.workspacePath === workspacePath &&
      proc.status !== "exited" &&
      isDefaultShellCommand(proc.command),
  );
}

/** A missing registry file means another view already cleared this proc. */
export function isMissingProcError(error: unknown): boolean {
  return /\bno such proc\b/i.test(String(error));
}
