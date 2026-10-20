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

  it("keeps only the confirmation line of a started `task kickoff`, raw answers otherwise", () => {
    const started = "ok: 老板点了「开工」——任务 t1 定为研究型。\n\n## 本任务的流程：研究";
    expect(classifyResult("task", "kickoff", started, false)).toEqual({
      kind: "confirm",
      text: "ok: 老板点了「开工」——任务 t1 定为研究型。",
    });
    const answered = '{"q":"还没聊完"}';
    expect(classifyResult("task", "kickoff", answered, false)).toEqual({ kind: "raw", text: answered });
  });
});
