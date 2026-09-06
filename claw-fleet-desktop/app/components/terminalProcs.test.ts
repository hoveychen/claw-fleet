import { describe, expect, it } from "vitest";
import type { ProcRecord } from "../types";
import { isMissingProcError, terminalProcsForWorkspace } from "./terminalProcs";

function proc(
  id: string,
  workspacePath: string,
  command: string,
  status: ProcRecord["status"],
): ProcRecord {
  return {
    id,
    workspacePath,
    command,
    status,
    startedMs: Number(id.replace(/\D/g, "")) || 0,
    cols: 80,
    rows: 24,
  };
}

describe("terminalProcsForWorkspace", () => {
  it("只展示当前仓库仍存活的交互式 shell", () => {
    const records = [
      proc("p4", "/repo", 'exec "/bin/zsh" -i', "running"),
      proc("p3", "/repo", "./scripts/build-local.sh", "running"),
      proc("p2", "/repo", 'exec "/bin/zsh" -i', "exited"),
      proc("p1", "/other", 'exec "/bin/zsh" -i', "running"),
    ];

    expect(terminalProcsForWorkspace(records, "/repo").map((p) => p.id)).toEqual(["p4"]);
  });

  it("保留 Windows 默认交互 shell", () => {
    const records = [proc("p1", "C:\\repo", "cmd", "running")];
    expect(terminalProcsForWorkspace(records, "C:\\repo")).toEqual(records);
  });
});

describe("isMissingProcError", () => {
  it("识别后端已经删除的 proc 记录", () => {
    expect(
      isMissingProcError(
        "no such proc p1a072dfa4082781: No such file or directory (os error 2)",
      ),
    ).toBe(true);
  });

  it("不把临时网络错误误判成记录已删除", () => {
    expect(isMissingProcError("rca remote recv failed: connection reset")).toBe(false);
  });
});
