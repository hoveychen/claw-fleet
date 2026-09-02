import { describe, expect, it } from "vitest";
import { formatBoundaryDetail } from "./ErrorBoundary";

// 崩溃详情是事后唯一能拿到的线索(手机上没有控制台)。这里钉住它的形状,免得
// 哪天「顺手简化」把 componentStack 或 stack 丢掉 —— 少了 componentStack 就
// 不知道是**哪个组件**抛的,而那恰恰是这两次白屏里最关键的一条信息。
//
// 组件本身的行为(捕获、爆炸半径、resetKey 自动恢复)不在这里验:本包的测试环境
// 是 node、没有 jsdom / testing-library,而 error boundary 在 SSR 下压根不工作。
// 那部分走 P3 的浏览器端到端(真 relay + 畸形卡),比装两个新依赖更贴近真相。
describe("formatBoundaryDetail", () => {
  it("带上错误名、消息、堆栈与组件栈", () => {
    const e = new TypeError("x.filter is not a function");
    e.stack = "TypeError: x.filter is not a function\n    at toolChoicesForSources";
    const out = formatBoundaryDetail(e, "\n    at NewSessionSheet\n    at App");

    expect(out).toContain("TypeError: x.filter is not a function");
    expect(out).toContain("at toolChoicesForSources");
    expect(out).toContain("Component stack:");
    expect(out).toContain("at NewSessionSheet");
  });

  it("没有组件栈时不留下空的 Component stack 段", () => {
    const e = new Error("boom");
    e.stack = "Error: boom";
    const out = formatBoundaryDetail(e, null);
    expect(out).not.toContain("Component stack");
    expect(out).toContain("Error: boom");
  });

  it("堆栈缺失时显式标注,而不是渲染 undefined", () => {
    const e = new Error("no stack here");
    e.stack = undefined;
    expect(formatBoundaryDetail(e, null)).toContain("(no stack)");
  });
});
