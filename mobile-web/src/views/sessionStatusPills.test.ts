// Core design of this row: **absence takes no space**.
//
// Old header panel is a fixed 5-row table; a session with no model yet shows "model —" on it.
// This row flips it — quiet sessions draw zero pills, whole row unrendered. Not an implementation detail,
// but a prerequisite for "information density" from the three requirements: only when silent fields truly
// vanish does that horizontal space free up for watch/subagent/plan progress. So each pill here is pinned
// on "when NOT to appear", not just "what to look like when appearing".

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
    // Give one field at a time, confirm each brings only its own pill.
    expect(keys(session({ status: "thinking" }))).toEqual(["running"]);
    expect(keys(session({ runningSubagentCount: 3 }))).toEqual(["subagents"]);
    expect(keys(session({ taskPlan: { done: 3, total: 5 } as never }))).toEqual(["plan"]);
    expect(keys(session({ contextPercent: 0.4 }))).toEqual(["context"]);
    // 0 subagents and 0-task plans both count as "absent", take no space.
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
      session({ watches: [{ id: "w1", created: 0, pollSecs: 30, deadlineAt: 0, pollCount: 12, structuralFailStreak: 0 }] }),
    );
    expect(one[0].label).toContain("12");
    const two = buildStatusPills(
      session({
        watches: [
          { id: "w1", created: 0, pollSecs: 30, deadlineAt: 0, pollCount: 12, structuralFailStreak: 0 },
          { id: "w2", created: 0, pollSecs: 30, deadlineAt: 0, pollCount: 3, structuralFailStreak: 0 },
        ],
      }),
    );
    expect(two[0].label).toContain("2");
    expect(two[0].label).not.toContain("12");
  });

  it("a watch whose until cannot run alerts instead of reporting a poll count", () => {
    // 204 polls, all exit 127 — this is not "waited a long time", it is a watch
    // that will never fire.
    const pills = buildStatusPills(
      session({
        watches: [
          {
            id: "w1",
            created: 0,
            pollSecs: 30,
            deadlineAt: 0,
            pollCount: 204,
            structuralFailStreak: 204,
            lastStderr: "sh: gh: command not found",
          },
        ],
      }),
    );
    const watch = pills.find((p) => p.key === "watch");
    expect(watch?.tone).toBe("alert");
    expect(watch?.label).not.toContain("204 次");
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
    // "running" and "N queued" are status statements with no corresponding detail pane, so not tappable.
    expect(target("running")).toBeUndefined();
    expect(target("queued")).toBeUndefined();
  });
});
