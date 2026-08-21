import { describe, expect, it } from "vitest";
import {
  reconcileDecisions,
  ANSWER_GRACE_MS,
  EMPTY_SNAPSHOT_COOLDOWN_MS,
} from "./decisionReconcile";
import type { PendingDecision } from "./types";

// 造一张最小决策卡；kind/request 内容不影响 reconcile，只看 id/arrivedAt。
function card(id: string, arrivedAt = 1_000): PendingDecision {
  return {
    kind: "guard",
    id,
    request: { id, command: "echo", sessionId: "s1" } as unknown as PendingDecision["request"],
    arrivedAt,
  };
}

describe("reconcileDecisions", () => {
  it("正常快照：直接替换成快照内容，并保留旧卡的 arrivedAt 不重排", () => {
    const prev = [card("a", 100), card("b", 200)];
    const fresh = [card("b", 999), card("c", 300)];
    const { decisions } = reconcileDecisions({
      prev,
      fresh,
      answeredAt: new Map(),
      now: 10_000,
    });
    expect(decisions.map((d) => d.id)).toEqual(["b", "c"]);
    // b 是旧卡：沿用旧的 arrivedAt(200)，不是快照里的 999。
    expect(decisions.find((d) => d.id === "b")!.arrivedAt).toBe(200);
  });

  // 这条复现老板报的 bug：网络差时本机刚回复过的卡，桌面端还没处理完，
  // 兜底 refresh 拉到的快照仍然带着它 —— 不能让它再次冒出来。
  it("本机刚回复、答复还在途中：快照仍带该卡时不得让它重新出现", () => {
    const now = 50_000;
    const answeredAt = new Map([["a", now - 2_000]]); // 2s 前刚回复，仍在宽限期内
    const { decisions } = reconcileDecisions({
      prev: [card("b")],
      fresh: [card("a"), card("b")], // 桌面端还没消化答复，快照里 a 还在
      answeredAt,
      now,
    });
    expect(decisions.map((d) => d.id)).toEqual(["b"]);
  });

  it("答复已落地（快照不再含该卡）：从 answeredAt 里清掉，卡保持消失", () => {
    const now = 50_000;
    const answeredAt = new Map([["a", now - 2_000]]);
    const { decisions, answeredAt: next } = reconcileDecisions({
      prev: [card("b")],
      fresh: [card("b")], // 桌面端已处理 a，快照里没有了
      answeredAt,
      now,
    });
    expect(decisions.map((d) => d.id)).toEqual(["b"]);
    expect(next.has("a")).toBe(false); // 已确认送达，抑制记录清除
  });

  it("超过宽限期仍在快照里：判定答复丢失，让卡重新出现以便重答", () => {
    const now = 50_000;
    const answeredAt = new Map([["a", now - ANSWER_GRACE_MS - 1]]); // 早已过宽限期
    const { decisions, answeredAt: next } = reconcileDecisions({
      prev: [],
      fresh: [card("a")],
      answeredAt,
      now,
    });
    expect(decisions.map((d) => d.id)).toEqual(["a"]);
    expect(next.has("a")).toBe(false); // 抑制记录清除，之后不再压制
  });
});

// ── 幽灵 agent 的空快照 ───────────────────────────────────────────────────────
// relay 把手机的每个请求广播给频道内**所有** agent，手机采信最先到的那个回复。
// 2026-08-21 实测：桌面端之外还有第二个 agent 反复短暂入频道（relay 日志 6 次
// Agent joined/left，每次约 20s），它跑在被重定向的 FLEET_HOME 下，于是
// pending_snapshot 回的是一份空列表。快照在客户端是权威的整表覆盖，卡片因此
// 被抹掉，等幽灵退出后下一轮 reconcile 才拉回来 —— 就是老板看到的
// 「卡片闪一下自己关了，十几秒后又出现」。
describe("reconcileDecisions —— 陌生 agent 的空快照不得抹掉本地卡片", () => {
  it("空快照且来源 agent 与主 agent 不一致：整份忽略，卡片留在原地", () => {
    const prev = [card("a", 1_000)];
    const r = reconcileDecisions({
      prev,
      fresh: [],
      answeredAt: new Map(),
      now: 900_000, // 卡片早就不新了，靠的是来源不符而非冷却
      agentKey: "ghost-host/pid=999/home=/tmp/x",
      trustedAgentKey: "desk-host/pid=18953/home=/Users/hoveychen",
    });
    expect(r.decisions.map((d) => d.id)).toEqual(["a"]);
    expect(r.ignored?.reason).toBe("foreign-empty-snapshot");
    expect(r.ignored?.agentKey).toBe("ghost-host/pid=999/home=/tmp/x");
    // 被忽略的快照不得改写主 agent —— 否则幽灵一来就篡位，下一份真快照反被当外人。
    expect(r.trustedAgentKey).toBe("desk-host/pid=18953/home=/Users/hoveychen");
  });

  it("空快照但没有指纹（老桌面端）：冷却期内的新卡仍受保护", () => {
    const now = 50_000;
    const r = reconcileDecisions({
      prev: [card("a", now - 1_000)], // 1s 前刚到的卡
      fresh: [],
      answeredAt: new Map(),
      now,
    });
    expect(r.decisions.map((d) => d.id)).toEqual(["a"]);
    expect(r.ignored?.reason).toBe("fresh-card-cooldown");
  });

  it("空快照来自主 agent 本人：照旧采信，卡片清空", () => {
    const key = "desk-host/pid=18953/home=/Users/hoveychen";
    const r = reconcileDecisions({
      prev: [card("a", 49_000)],
      fresh: [],
      answeredAt: new Map(),
      now: 50_000, // 卡很新，但来源可信 —— 冷却不该拦住权威的「已清空」
      agentKey: key,
      trustedAgentKey: key,
    });
    expect(r.decisions).toEqual([]);
    expect(r.ignored).toBeUndefined();
  });

  it("空快照无指纹且卡片已过冷却期：仍然采信，不会永久卡住", () => {
    const now = 50_000;
    const r = reconcileDecisions({
      prev: [card("a", now - EMPTY_SNAPSHOT_COOLDOWN_MS - 1)],
      fresh: [],
      answeredAt: new Map(),
      now,
    });
    expect(r.decisions).toEqual([]);
    expect(r.ignored).toBeUndefined();
  });

  it("非空快照：采信内容，并把来源记为主 agent", () => {
    const r = reconcileDecisions({
      prev: [],
      fresh: [card("a")],
      answeredAt: new Map(),
      now: 10_000,
      agentKey: "desk-host/pid=18953/home=/Users/hoveychen",
    });
    expect(r.decisions.map((d) => d.id)).toEqual(["a"]);
    expect(r.trustedAgentKey).toBe("desk-host/pid=18953/home=/Users/hoveychen");
    expect(r.ignored).toBeUndefined();
  });
});
