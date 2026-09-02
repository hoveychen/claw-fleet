import { describe, expect, it } from "vitest";
import { parsePairingLink } from "./pairingLink";

const SECRET = "b8c0de1f2a3b4c5d6e7f8091a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d5";

describe("parsePairingLink", () => {
  it("自建 relay:origin 就是这台设备该连的地址", () => {
    expect(parsePairingLink(`https://relay.corp.example.com/#k=${SECRET}`)).toEqual({
      secret: SECRET,
      relayBase: "https://relay.corp.example.com",
    });
  });

  it("局域网自建 relay(http + 端口)照样认", () => {
    expect(parsePairingLink(`http://192.168.1.9:18080/#k=${SECRET}`)).toEqual({
      secret: SECRET,
      relayBase: "http://192.168.1.9:18080",
    });
  });

  it("桌面端附带的 &lang= 不影响解析", () => {
    expect(parsePairingLink(`https://fleet-relay.muveeai.com/#k=${SECRET}&lang=zh`)).toEqual({
      secret: SECRET,
      relayBase: "https://fleet-relay.muveeai.com",
    });
  });

  it("显式 &relay= 胜过 origin(鸿蒙壳写的形状)", () => {
    expect(
      parsePairingLink(
        `https://fleet.local/index.html#k=${SECRET}&relay=${encodeURIComponent("https://r.example.com")}`,
      ),
    ).toEqual({ secret: SECRET, relayBase: "https://r.example.com" });
  });

  // 用户粘贴时最可能出的岔子:前后带空白、贴了半截、贴成了别的东西。
  it("前后空白照常吃掉", () => {
    expect(parsePairingLink(`  https://r.example.com/#k=${SECRET}\n`)).toEqual({
      secret: SECRET,
      relayBase: "https://r.example.com",
    });
  });

  it("没有 #k= 的链接不是配对链接", () => {
    expect(parsePairingLink("https://fleet-relay.muveeai.com/")).toBeNull();
  });

  it("空输入不是配对链接", () => {
    expect(parsePairingLink("   ")).toBeNull();
  });

  // relay 的 auth 帧要求 >= 16;短的当场拒掉,好过配上去之后卡在连不上。
  it("太短的密钥拒掉", () => {
    expect(parsePairingLink("https://r.example.com/#k=deadbeef")).toBeNull();
  });

  // `?k=` 是刻意不认的:query 会进 relay 的访问日志,而 relay 必须看不到密钥。
  it("查询串里的 k= 不认(密钥只能走 fragment)", () => {
    expect(parsePairingLink(`https://r.example.com/?k=${SECRET}`)).toBeNull();
  });
});
