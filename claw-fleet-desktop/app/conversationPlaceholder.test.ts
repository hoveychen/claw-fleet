import { describe, expect, it } from "vitest";
import { conversationPlaceholder } from "./conversationPlaceholder";

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

  it("空会话、也不在取数时,交给正常的空态渲染", () => {
    expect(conversationPlaceholder({ isLoading: false, stalled: false, messageCount: 0 }))
      .toBeNull();
  });
});
