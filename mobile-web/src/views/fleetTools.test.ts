import { describe, expect, it } from "vitest";

import { classifyResult } from "./fleetTools";

describe("classifyResult", () => {
  it("keeps only the confirmation line of `task set`, dropping the workflow block", () => {
    const text = "ok: task t1 saved (type=research, goal=g)\n\n## 本任务的流程：研究\n- 开工先发计划卡";
    expect(classifyResult("task", "set", text, false)).toEqual({
      kind: "confirm",
      text: "ok: task t1 saved (type=research, goal=g)",
    });
  });
});
