import { describe, expect, it } from "vitest";
import {
  defaultRecoverySelection,
  recoverableCount,
  recoveryRows,
  selectedRecoveryIds,
  type RecoveryClass,
  type SubtaskStatus,
} from "../../shared-ts/subtaskRecovery";

const st = (sessionId: string, cls: RecoveryClass): SubtaskStatus => ({
  sessionId,
  effectiveId: sessionId,
  spawnedBy: "coord",
  phase: "failed",
  class: cls,
  detail: "",
});

describe("subtask recovery list", () => {
  const list = [
    st("done", "done"),
    st("auth", "authFailed"),
    st("a", "recoverable"),
    st("closed", "closed"),
    st("esc", "userInterrupted"),
    st("b", "recoverable"),
    st("run", "running"),
  ];

  it("counts only recoverable sub-tasks", () => {
    expect(recoverableCount(list)).toBe(2);
    expect(recoverableCount(null)).toBe(0);
  });

  it("lists recoverable first, greys failures, hides done/closed/running", () => {
    const rows = recoveryRows(list);
    expect(rows.map((r) => [r.status.sessionId, r.selectable])).toEqual([
      ["a", true],
      ["b", true],
      ["auth", false],
      ["esc", false],
    ]);
  });

  it("pre-ticks recoverable ones and never sends a greyed row", () => {
    const rows = recoveryRows(list);
    const picked = defaultRecoverySelection(list);
    expect([...picked].sort()).toEqual(["a", "b"]);
    picked.add("auth");
    picked.add("done");
    picked.delete("a");
    expect(selectedRecoveryIds(rows, picked)).toEqual(["b"]);
  });
});
