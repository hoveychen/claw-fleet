import { describe, expect, it } from "vitest";
import { isDefaultShellCommand, procCommandText, procLabel } from "./procCommandLabel";
import type { ProcRecord } from "../types";

/**
 * These assertions exist because of a real regression: when core's default_shell_command
 * changed from `exec "$SHELL" -i` to `exec "/bin/zsh" -i`, the regex for recognizing
 * commands didn't update, so the default shell label showed as "exec". At that time,
 * two pages each had their own copy of the regex, and only one was synced.
 *
 * So we lock in **both forms must be recognized** here — the new form is what's currently
 * generated, and the old form exists in still-alive pty records from before that change.
 *
 * isDefaultShellCommand now lives in shared-ts/procShell.ts, and the mobile end imports
 * the same file (re-exported through this module). So this test group passing means both
 * ends pass — the shared layer doesn't have its own vitest project, so we put test cases
 * here to avoid spinning up another test runner for a pure function.
 */

function rec(command: string): ProcRecord {
  return {
    id: "p1",
    workspacePath: "/repo",
    command,
    status: "running",
    startedMs: 0,
    cols: 80,
    rows: 24,
  } as ProcRecord;
}

describe("isDefaultShellCommand", () => {
  it("recognizes the absolute path form that core currently generates", () => {
    expect(isDefaultShellCommand('exec "/bin/zsh" -i')).toBe(true);
    expect(isDefaultShellCommand('exec "/usr/local/bin/fish" -i')).toBe(true);
    expect(isDefaultShellCommand('  exec "/bin/sh" -i  ')).toBe(true);
  });

  it("recognizes the old form left from before the change", () => {
    expect(isDefaultShellCommand('exec "$SHELL" -i')).toBe(true);
    expect(isDefaultShellCommand("exec $SHELL -i")).toBe(true);
  });

  it("recognizes Windows bare cmd", () => {
    expect(isDefaultShellCommand("cmd")).toBe(true);
  });

  it("doesn't misclassify normal commands as shell", () => {
    expect(isDefaultShellCommand("pnpm build")).toBe(false);
    expect(isDefaultShellCommand("cargo test")).toBe(false);
    // Similar in appearance but not actually: exec a script, no -i
    expect(isDefaultShellCommand('exec "/bin/zsh" script.sh')).toBe(false);
    expect(isDefaultShellCommand("cmd /c dir")).toBe(false);
  });
});

describe("procLabel", () => {
  it("default shell uses given short name", () => {
    expect(procLabel(rec('exec "/bin/zsh" -i'), "shell")).toBe("shell");
  });

  it("others take first word", () => {
    expect(procLabel(rec("pnpm build --watch"), "shell")).toBe("pnpm");
  });

  it("truncates overly long first word", () => {
    const label = procLabel(rec("./scripts/run-a-very-long-thing.sh"), "shell");
    expect(label).toHaveLength(16);
    expect(label.endsWith("…")).toBe(true);
  });
});

describe("procCommandText", () => {
  it("default shell becomes short name", () => {
    expect(procCommandText('exec "/bin/zsh" -i', "shell")).toBe("shell");
  });

  it("normal commands keep full text — the command panel's value is seeing which command is actually running", () => {
    expect(procCommandText("pnpm build --watch", "shell")).toBe("pnpm build --watch");
  });
});
