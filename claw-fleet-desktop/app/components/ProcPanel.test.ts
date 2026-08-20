import { describe, expect, it } from "vitest";
import type { ProcRecord } from "../types";
import { groupProcs, parseProcShortcuts, shortcutsFor } from "./ProcPanel";

function proc(id: string, command: string, status: ProcRecord["status"]): ProcRecord {
  return {
    id,
    workspacePath: "/repo",
    command,
    status,
    startedMs: Number(id),
    cols: 80,
    rows: 24,
  };
}

describe("repository command groups", () => {
  it("groups exact commands and preserves newest-first execution order", () => {
    const groups = groupProcs([
      proc("3", "pnpm test", "running"),
      proc("2", "pnpm lint", "exited"),
      proc("1", "pnpm test", "exited"),
    ]);

    expect(groups.map((group) => group.command)).toEqual(["pnpm test", "pnpm lint"]);
    expect(groups[0].records.map((record) => record.id)).toEqual(["3", "1"]);
    expect(groups[0].runningCount).toBe(1);
  });

  it("only merges commands with identical text", () => {
    expect(groupProcs([
      proc("2", "pnpm test", "exited"),
      proc("1", "pnpm  test", "exited"),
    ])).toHaveLength(2);
  });
});

describe("command shortcuts", () => {
  it("loads unique, non-empty commands per repository", () => {
    const map = parseProcShortcuts(
      '{"/repo":["pnpm test","","pnpm test","pnpm dev"],"/other":["cargo build"]}',
    );
    expect(shortcutsFor(map, "/repo")).toEqual(["pnpm test", "pnpm dev"]);
    expect(shortcutsFor(map, "/other")).toEqual(["cargo build"]);
  });

  // The bug this shape exists to prevent: a pin made in one repository must not
  // show up (and be runnable at the wrong cwd) in every other repository.
  it("keeps a pin out of repositories it was not pinned in", () => {
    const map = parseProcShortcuts('{"/repo":["./scripts/build-local.sh"]}');
    expect(shortcutsFor(map, "/other")).toEqual([]);
  });

  it("drops the legacy global array instead of leaking it everywhere", () => {
    const map = parseProcShortcuts('["./scripts/build-local.sh"]');
    expect(shortcutsFor(map, "/repo")).toEqual([]);
    expect(shortcutsFor(map, "/other")).toEqual([]);
  });

  it("tolerates corrupt persisted data", () => {
    expect(parseProcShortcuts("not json")).toEqual({});
    expect(parseProcShortcuts('{"/repo":"pnpm test"}')).toEqual({});
  });
});
