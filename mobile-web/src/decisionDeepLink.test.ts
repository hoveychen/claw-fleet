import { describe, expect, it } from "vitest";
import { parseDecisionDeepLink } from "./decisionDeepLink";

describe("parseDecisionDeepLink", () => {
  // 三条投递路径给的形状不一样:地址栏是完整 URL,SW 转发的是 notify 里原样的
  // `/#d=...`,原生壳给的也是后者。都得认。
  it("完整 URL", () => {
    expect(parseDecisionDeepLink("https://fleet.local/index.html#d=guard:g1")).toEqual({
      kind: "guard",
      id: "g1",
    });
  });

  it("notify 原样的路径形式", () => {
    expect(parseDecisionDeepLink("/#d=elicitation:e-1")).toEqual({
      kind: "elicitation",
      id: "e-1",
    });
  });

  // id 是外部给的,不保证不含冒号;kind 不含。所以按第一个冒号切,剩下的全归 id。
  it("id 里含冒号时只按第一个冒号切", () => {
    expect(parseDecisionDeepLink("/#d=guard:a:b:c")).toEqual({ kind: "guard", id: "a:b:c" });
  });

  it("没有 fragment → null", () => {
    expect(parseDecisionDeepLink("/")).toBeNull();
    expect(parseDecisionDeepLink("https://fleet.local/")).toBeNull();
  });

  // 配对密钥走的是 `#k=`(见 secretStore),绝不能被当成决策目标。
  it("别的 fragment 不误认", () => {
    expect(parseDecisionDeepLink("/#k=SECRET")).toBeNull();
  });

  // 桌面在请求没带 id 时会把 tag 退化成裸 kind,那种链接没有可聚焦的目标。
  it("只有 kind 没有 id → null", () => {
    expect(parseDecisionDeepLink("/#d=guard")).toBeNull();
    expect(parseDecisionDeepLink("/#d=guard:")).toBeNull();
    expect(parseDecisionDeepLink("/#d=")).toBeNull();
    expect(parseDecisionDeepLink("/#d=:g1")).toBeNull();
  });
});
