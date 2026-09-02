import { describe, expect, it } from "vitest";
import {
  defaultRelayBaseFrom,
  parseRelayParam,
  relayBaseFor,
  relayWsUrl,
} from "./relayBase";

// `resolveRelayBase` / `relayDisplayHost` 的用例仍在 relay.test.ts —— 它们跟着
// 函数一起搬过来太吵，改动只是导入路径。这里补的是多设备新长出来的那几个。

const BAKED = "https://fleet-relay.muveeai.com";

describe("parseRelayParam", () => {
  it("reads the relay a pairing QR named", () => {
    expect(parseRelayParam("#k=abc&relay=https%3A%2F%2Frelay.corp.example.com")).toBe(
      "https://relay.corp.example.com",
    );
  });

  it("drops the path — a relay behind a prefix is unsupported everywhere in this client", () => {
    expect(parseRelayParam("#relay=https%3A%2F%2Fr.example.com%2Fprefix%2F")).toBe(
      "https://r.example.com",
    );
  });

  it("keeps an explicit port (the dev relay case)", () => {
    expect(parseRelayParam("#relay=http%3A%2F%2F127.0.0.1%3A18080")).toBe(
      "http://127.0.0.1:18080",
    );
  });

  it("returns null when no relay is named", () => {
    expect(parseRelayParam("#k=abc")).toBeNull();
    expect(parseRelayParam("")).toBeNull();
  });

  // 二维码是不可信输入，而这个值会成为客户端后续每个 URL 的基底。
  it("refuses anything that is not an absolute http(s) URL", () => {
    expect(parseRelayParam("#relay=javascript%3Aalert(1)")).toBeNull();
    expect(parseRelayParam("#relay=file%3A%2F%2F%2Fetc%2Fpasswd")).toBeNull();
    expect(parseRelayParam("#relay=%2Fjust%2Fa%2Fpath")).toBeNull();
    expect(parseRelayParam("#relay=not%20a%20url")).toBeNull();
  });
});

describe("defaultRelayBaseFrom", () => {
  it("prefers the baked build constant over the page origin", () => {
    // 鸿蒙壳的页面 origin 是假域名 https://fleet.local，退回它会让 app 拨自己。
    expect(defaultRelayBaseFrom(BAKED, "https://fleet.local")).toBe(BAKED);
  });

  it("falls back to the origin — the PWA case, where the relay serves the page", () => {
    expect(defaultRelayBaseFrom(undefined, BAKED)).toBe(BAKED);
    expect(defaultRelayBaseFrom("", BAKED)).toBe(BAKED);
  });
});

describe("relayBaseFor", () => {
  it("uses the relay the device named", () => {
    expect(relayBaseFor("https://relay.corp.example.com")).toBe(
      "https://relay.corp.example.com",
    );
  });

  // 迁移过来的设备与扫码时没带 &relay= 的设备都是 null —— 那不是「未知」，
  // 而是「就用构建默认值」（本测试环境里 origin 是 http://localhost）。
  it("falls back to the build default when the device named none", () => {
    expect(relayBaseFor(null)).toBe("http://localhost");
    expect(relayBaseFor(undefined)).toBe("http://localhost");
  });
});

describe("relayWsUrl", () => {
  it("maps http(s) to ws(s) and appends /ws", () => {
    expect(relayWsUrl("https://r.example.com")).toBe("wss://r.example.com/ws");
    expect(relayWsUrl("http://127.0.0.1:18080")).toBe("ws://127.0.0.1:18080/ws");
  });

  it("tolerates a trailing slash", () => {
    expect(relayWsUrl("https://r.example.com/")).toBe("wss://r.example.com/ws");
  });
});
