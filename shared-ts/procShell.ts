// Shared check between desktop and mobile: "is this command the default shell?"
//
// This directory is the only cross-frontend-package shared code point in the repo. claw-fleet-desktop
// and mobile-web are two independent pnpm packages (no workspace protocol, no shared npm packages), so
// sharing works by both having tsconfig include this directory and importing via relative paths.
//
// Only **zero-dependency pure functions** go here. Anything needing ProcRecord, i18n, or React doesn't
// belong — the two packages' ProcRecord types and t() mechanisms differ, pulling across would freeze
// both type systems together. For shell specifically: both sides keep their own procLabel, sharing only
// this string check.

/** Is this command core's "give me a terminal" default shell?
 *
 *  Must recognize both shapes: core now generates `exec "<absolute-path>" -i` (see
 *  proc_runner::default_shell_command), and pty records still alive from before that change store
 *  the old `exec "$SHELL" -i`. Windows side is bare `cmd`.
 *
 *  Worth cross-package sharing because this check used to live separately on desktop and mobile; when
 *  core changed the default from `$SHELL` to absolute path, only one copy was synced, the other started
 *  showing default shell as "exec". */
export function isDefaultShellCommand(command: string): boolean {
  const cmd = command.trim();
  return /^exec\s+"[^"]*"\s+-i$/.test(cmd) || /^exec\s+"?\$SHELL/.test(cmd) || cmd === "cmd";
}
