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

// relay 在扇出通知时会把来源 channel 盖进点击目标(fleet-relay 的
// notify_target.rs)。没有它,两台机器同时有卡时点开落到哪一张全靠运气。
describe("channel mark", () => {
  it("reads the source channel relay stamped on", () => {
    const target = parseDecisionDeepLink("/#d=guard:g1&ch=105e300f");
    expect(target).toEqual({ kind: "guard", id: "g1", channelMark: "105e300f" });
  });

  // 关键回归:老的解析是「前缀 d= 之后全是 id」,那会把 &ch=… 一起吞进 id,
  // 于是**每一条**带标记的通知都点不开(id 对不上任何一张卡)。
  it("does not swallow the mark into the card id", () => {
    expect(parseDecisionDeepLink("/#d=guard:g1&ch=105e300f")?.id).toBe("g1");
  });

  it("still parses a link from a relay that stamps nothing", () => {
    expect(parseDecisionDeepLink("/#d=guard:g1")).toEqual({ kind: "guard", id: "g1" });
  });

  it("tolerates the mark coming first", () => {
    expect(parseDecisionDeepLink("/#ch=105e300f&d=fleet-ask:abc")).toEqual({
      kind: "fleet-ask",
      id: "abc",
      channelMark: "105e300f",
    });
  });

  it("ignores a link that only carries a mark", () => {
    expect(parseDecisionDeepLink("/#ch=105e300f")).toBeNull();
  });
});
