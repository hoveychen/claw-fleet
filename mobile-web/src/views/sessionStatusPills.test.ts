// 钉住这条轨的核心取舍：**缺席的东西不占位**。
//
// 旧头部那块面板是固定五行的表格，一个还没记到模型的会话在上面显示「模型 —」；
// 这条轨反过来——安静的会话上它一颗 pill 都不画，整条不渲染。这不是实现细节，
// 是老板那三条意见里「信息量」那条能成立的前提：只有让沉默的字段真正消失，
// 那点横向空间才腾得出来给 watch / 子代理 / 计划进度。所以这里逐条钉的是
// 「什么时候不出现」，而不只是「出现时长什么样」。

import { describe, it, expect } from "vitest";
import { buildStatusPills } from "./sessionStatusPills";
import type { SessionInfo, SessionStatus } from "../types";

function session(over: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id: "s1",
    workspacePath: "/w",
    workspaceName: "w",
    status: "idle" as SessionStatus,
    isSubagent: false,
    lastActivityMs: 0,
    createdAtMs: 0,
    jsonlPath: "/w/s1.jsonl",
    ...over,
  } as SessionInfo;
}

const keys = (s: SessionInfo, pendingDecisions = 0) =>
  buildStatusPills(s, { pendingDecisions }).map((p) => p.key);

describe("buildStatusPills", () => {
  it("安静的会话上整条轨是空的", () => {
    expect(buildStatusPills(session())).toEqual([]);
  });

  it("每个字段缺席时它那颗 pill 不出现（不是显示成空值）", () => {
    // 只给一个字段，逐个确认它只带出自己那一颗。
    expect(keys(session({ status: "thinking" }))).toEqual(["running"]);
    expect(keys(session({ runningSubagentCount: 3 }))).toEqual(["subagents"]);
    expect(keys(session({ taskPlan: { done: 3, total: 5 } as never }))).toEqual(["plan"]);
    expect(keys(session({ contextPercent: 0.4 }))).toEqual(["context"]);
    // 0 个子代理与 0 个任务的计划都算「没有」，不占位。
    expect(keys(session({ runningSubagentCount: 0 }))).toEqual([]);
    expect(keys(session({ taskPlan: { done: 0, total: 0 } as never }))).toEqual([]);
    expect(keys(session({ pendingMessages: [] }))).toEqual([]);
    expect(keys(session({ watches: [] }))).toEqual([]);
  });

  it("挡路的排在动态之前，动态排在读数之前", () => {
    const s = session({
      status: "executing",
      outOfCredits: "out of credits",
      runningSubagentCount: 2,
      contextPercent: 0.4,
      totalCostUsd: 43.27,
    });
    expect(keys(s, 2)).toEqual([
      "outOfCredits",
      "decisions",
      "running",
      "subagents",
      "context",
      "cost",
    ]);
  });

  it("挡路的那几颗是 alert，在动的是 live，读数是 neutral", () => {
    const byKey = new Map(
      buildStatusPills(
        session({
          status: "streaming",
          remoteDisconnect: { host: "h", reason: "r" } as never,
          contextPercent: 0.4,
        }),
        { pendingDecisions: 1 },
      ).map((p) => [p.key, p]),
    );
    expect(byKey.get("remoteDisconnect")?.tone).toBe("alert");
    expect(byKey.get("decisions")?.tone).toBe("alert");
    expect(byKey.get("running")?.tone).toBe("live");
    expect(byKey.get("context")?.tone).toBe("neutral");
  });

  it("只有「运行中」那颗带圆点——它接替旧头部右上角那个脉冲点", () => {
    const pills = buildStatusPills(
      session({ status: "thinking", runningSubagentCount: 1, contextPercent: 0.5 }),
    );
    expect(pills.filter((p) => p.dot).map((p) => p.key)).toEqual(["running"]);
  });

  it("waitingInput 不算在动——那是停下来等人", () => {
    expect(keys(session({ status: "waitingInput" }))).toEqual([]);
  });

  it("单个 watch 报轮询次数，多个报个数", () => {
    const one = buildStatusPills(
      session({ watches: [{ id: "w1", created: 0, pollSecs: 30, deadlineAt: 0, pollCount: 12 }] }),
    );
    expect(one[0].label).toContain("12");
    const two = buildStatusPills(
      session({
        watches: [
          { id: "w1", created: 0, pollSecs: 30, deadlineAt: 0, pollCount: 12 },
          { id: "w2", created: 0, pollSecs: 30, deadlineAt: 0, pollCount: 3 },
        ],
      }),
    );
    expect(two[0].label).toContain("2");
    expect(two[0].label).not.toContain("12");
  });

  it("半分钱以下的花费不占一颗 pill（$0.00 等于没说）", () => {
    expect(keys(session({ totalCostUsd: 0.004 }))).toEqual([]);
    expect(keys(session({ totalCostUsd: 0.005 }))).toEqual(["cost"]);
  });

  it("contextPercent 是 0–1 的比值，不是百分数", () => {
    const p = buildStatusPills(session({ contextPercent: 0.4 }))[0];
    expect(p.label).toContain("40");
  });

  it("能点的 pill 都指向一个存在的面，纯读数的不带 target", () => {
    const pills = buildStatusPills(
      session({
        status: "thinking",
        pendingMessages: ["a"],
        taskPlan: { done: 1, total: 4 } as never,
        handoff: { chainId: "c", hop: 3, chainLen: 4 },
        contextPercent: 0.2,
      }),
      { pendingDecisions: 1 },
    );
    const target = (k: string) => pills.find((p) => p.key === k)?.target;
    expect(target("decisions")).toBe("decisions");
    expect(target("plan")).toBe("plans");
    expect(target("handoff")).toBe("handoff");
    expect(target("context")).toBe("token");
    // 「运行中」和「N 条排队」是状态陈述，没有对应的详情面，所以不可点。
    expect(target("running")).toBeUndefined();
    expect(target("queued")).toBeUndefined();
  });
});
