import { describe, expect, it } from "vitest";
import { computeCongestion, reconnectLevel, rttLevel, worse } from "./connQuality";

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

  it("computeCongestion combines both signals by worst-of", () => {
    expect(computeCongestion(120, 0)).toBe("good");
    expect(computeCongestion(120, 1)).toBe("fair"); // reconnect dominates
    expect(computeCongestion(1500, 0)).toBe("congested"); // rtt dominates
    expect(computeCongestion(500, 3)).toBe("congested");
    expect(computeCongestion(null, 0)).toBe("good");
  });
});
