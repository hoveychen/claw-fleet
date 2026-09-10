import { describe, expect, it } from "vitest";
import { conversationPlaceholder, showLatestSync } from "./conversationPlaceholder";

describe("conversationPlaceholder", () => {
  it("转圈只在真的还在取数、且一条都没有时出现", () => {
    expect(conversationPlaceholder({ isLoading: true, stalled: false, messageCount: 0 }))
      .toBe("loading");
  });

  it("到期没拿到 transcript 就换成可解释、可重试的 stalled 面板", () => {
    expect(conversationPlaceholder({ isLoading: false, stalled: true, messageCount: 0 }))
      .toBe("stalled");
  });

  it("stalled 压过 loading —— 否则又变回一个永不结束的转圈", () => {
    expect(conversationPlaceholder({ isLoading: true, stalled: true, messageCount: 0 }))
      .toBe("stalled");
  });

  it("已经有消息可看时,后台再慢也不许占领整个面板", () => {
    expect(conversationPlaceholder({ isLoading: true, stalled: true, messageCount: 3 }))
      .toBeNull();
    expect(conversationPlaceholder({ isLoading: true, stalled: false, messageCount: 3 }))
      .toBeNull();
  });

  it("取数失败要报「失败」,不许冒充「超时」", () => {
    // 这正是老板看到的那一幕:后端秒回一个错误,界面却说「后端一直没有响应」。
    expect(
      conversationPlaceholder({ isLoading: false, stalled: false, failed: true, messageCount: 0 }),
    ).toBe("failed");
  });

  it("failed 压过 stalled —— 慢到超时最后又报错时,报错更具体", () => {
    expect(
      conversationPlaceholder({ isLoading: false, stalled: true, failed: true, messageCount: 0 }),
    ).toBe("failed");
  });

  it("已经有消息可看时,报错也不许占领整个面板", () => {
    expect(
      conversationPlaceholder({ isLoading: false, stalled: false, failed: true, messageCount: 3 }),
    ).toBeNull();
  });

  it("failed 缺省为 false,老调用点行为不变", () => {
    expect(conversationPlaceholder({ isLoading: false, stalled: true, messageCount: 0 }))
      .toBe("stalled");
  });

  it("空会话、也不在取数时,交给正常的空态渲染", () => {
    expect(conversationPlaceholder({ isLoading: false, stalled: false, messageCount: 0 }))
      .toBeNull();
  });
});

describe("showLatestSync", () => {
  it("有旧消息且正在刷新最新 tail 时明确提示", () => {
    expect(showLatestSync({ isLoading: true, isLoadingEarlier: false, messageCount: 3 }))
      .toBe(true);
  });

  it("首次加载与加载更早记录不冒充同步最新消息", () => {
    expect(showLatestSync({ isLoading: true, isLoadingEarlier: false, messageCount: 0 }))
      .toBe(false);
    expect(showLatestSync({ isLoading: true, isLoadingEarlier: true, messageCount: 3 }))
      .toBe(false);
  });
});
