import { describe, expect, it } from "vitest";
import {
  computeCongestion,
  formatRttSplit,
  reconnectLevel,
  rttLevel,
  splitRtt,
  worse,
} from "./connQuality";

describe("connQuality", () => {
  it("classifies RTT into three levels at the boundaries", () => {
    expect(rttLevel(null)).toBe("good");
    expect(rttLevel(120)).toBe("good");
    expect(rttLevel(300)).toBe("good");
    expect(rttLevel(301)).toBe("fair");
    expect(rttLevel(1200)).toBe("fair");
    expect(rttLevel(1201)).toBe("congested");
  });

  it("classifies reconnect counts", () => {
    expect(reconnectLevel(0)).toBe("good");
    expect(reconnectLevel(1)).toBe("fair");
    expect(reconnectLevel(2)).toBe("fair");
    expect(reconnectLevel(3)).toBe("congested");
  });

  it("worse() takes the higher-severity level, order-independent", () => {
    expect(worse("good", "fair")).toBe("fair");
    expect(worse("fair", "good")).toBe("fair");
    expect(worse("congested", "good")).toBe("congested");
    expect(worse("fair", "fair")).toBe("fair");
  });

  it("splitRtt attributes the total to the three places it can be spent", () => {
    // 620 total, 180 of it this phone's own link, 380 inside the desktop handler
    // ⇒ the 60ms left is relay↔desktop. This is the case the whole feature is
    // for: "slow" is the handler's fault, not either network's.
    expect(splitRtt({ totalMs: 620, phoneRelayMs: 180, desktopHandleMs: 380 })).toEqual({
      totalMs: 620,
      phoneMs: 180,
      desktopLinkMs: 60,
      handleMs: 380,
    });
  });

  it("splitRtt leaves the residual unknown when either measured segment is", () => {
    // No `msg_ack` (lost the race) or an older desktop (no handle_ms): the
    // residual is unknowable, and guessing 0 would blame a healthy link.
    expect(splitRtt({ totalMs: 500, phoneRelayMs: null, desktopHandleMs: 100 }).desktopLinkMs)
      .toBeNull();
    expect(splitRtt({ totalMs: 500, phoneRelayMs: 100, desktopHandleMs: null }).desktopLinkMs)
      .toBeNull();
  });

  it("splitRtt clamps a negative residual to zero", () => {
    // The two segments come off different clocks, so their sum can exceed the
    // total by a few ms. A negative leg would render as nonsense.
    expect(splitRtt({ totalMs: 100, phoneRelayMs: 80, desktopHandleMs: 40 }).desktopLinkMs).toBe(0);
  });

  it("formatRttSplit omits segments that were never measured", () => {
    const id = (s: string) => s;
    expect(
      formatRttSplit(splitRtt({ totalMs: 620, phoneRelayMs: 180, desktopHandleMs: 380 }), id),
    ).toBe("620ms · 手机 180 · 桌面链路 60 · 处理 380");
    // Nothing but the total is known — say only that, don't imply 0ms legs.
    expect(
      formatRttSplit(splitRtt({ totalMs: 90, phoneRelayMs: null, desktopHandleMs: null }), id),
    ).toBe("90ms");
    // Phone leg known, desktop silent: the residual is unknown, so only the one
    // segment we actually measured is named.
    expect(
      formatRttSplit(splitRtt({ totalMs: 90, phoneRelayMs: 40, desktopHandleMs: null }), id),
    ).toBe("90ms · 手机 40");
  });

  it("computeCongestion combines both signals by worst-of", () => {
    expect(computeCongestion(120, 0)).toBe("good");
    expect(computeCongestion(120, 1)).toBe("fair"); // reconnect dominates
    expect(computeCongestion(1500, 0)).toBe("congested"); // rtt dominates
    expect(computeCongestion(500, 3)).toBe("congested");
    expect(computeCongestion(null, 0)).toBe("good");
  });
});
