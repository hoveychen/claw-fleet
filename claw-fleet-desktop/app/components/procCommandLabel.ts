// How to display a workspace proc's command to the desktop user.
//
// Separated into a module because **two pages need it**: the terminal page's
// tab bar and the repo page's command panel execution history. The two functions
// below depend on this package's ProcRecord and the i18n copy passed by the
// caller, so they stay in this package; they both depend on a "is this the
// default shell" check that is cross-package shared to the mobile client (see
// shared-ts/).

import type { ProcRecord } from "../types";
import { isDefaultShellCommand } from "../../../shared-ts/procShell";

// Callers in this package (and their tests) need not know this check comes from outside.
export { isDefaultShellCommand };

/** Short name for terminal page tabs: default shell → `shellLabel`, others take
 *  the first word and truncate. */
export function procLabel(proc: ProcRecord, shellLabel: string): string {
  const cmd = proc.command.trim();
  if (isDefaultShellCommand(cmd)) return shellLabel;
  const head = cmd.split(/\s+/)[0] ?? cmd;
  return head.length > 16 ? `${head.slice(0, 15)}…` : head;
}

/** Full command text in the command panel execution history: default shell
 *  becomes `shellLabel`, others stay as-is. This does NOT truncate here —
 *  the value of the command panel is seeing exactly which command ran. */
export function procCommandText(command: string, shellLabel: string): string {
  return isDefaultShellCommand(command) ? shellLabel : command;
}
