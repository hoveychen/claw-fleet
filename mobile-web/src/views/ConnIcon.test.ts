import { describe, expect, it } from "vitest";
import { connIconKind } from "./ConnIcon";

describe("connIconKind", () => {
  it("链路没连上时,无论 agent 状态都是 connecting", () => {
    expect(connIconKind(false, false, "good")).toBe("connecting");
    expect(connIconKind(false, true, "congested")).toBe("connecting");
  });

  it("链路通了但桌面端不在,单独一种形态——不能混进信号强度", () => {
    expect(connIconKind(true, false, "good")).toBe("desktop-offline");
  });

  it("全通时由拥塞档决定亮几格", () => {
    expect(connIconKind(true, true, "good")).toBe("good");
    expect(connIconKind(true, true, "fair")).toBe("fair");
    expect(connIconKind(true, true, "congested")).toBe("congested");
  });
});
