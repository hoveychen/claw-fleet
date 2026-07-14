import { describe, expect, it } from "vitest";
import { reconcileDecisions, ANSWER_GRACE_MS } from "./decisionReconcile";
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
