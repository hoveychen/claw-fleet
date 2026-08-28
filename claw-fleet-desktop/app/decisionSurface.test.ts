import { describe, it, expect } from "vitest";
import { decisionSurface } from "./decisionSurface";

const DESKTOP = {
  webBuild: false,
  floatingPreferred: false,
  liteMode: false,
  mainMinimized: false,
};

describe("decisionSurface — desktop", () => {
  it("renders inline by default", () => {
    expect(decisionSurface(DESKTOP)).toBe("inline");
  });

  it("floats when the user asked for the standalone window", () => {
    expect(decisionSurface({ ...DESKTOP, floatingPreferred: true })).toBe("float");
  });

  it("floats in Lite mode, which draws no in-window card", () => {
    expect(decisionSurface({ ...DESKTOP, liteMode: true })).toBe("float");
  });

  it("floats while the main window is minimized", () => {
    expect(decisionSurface({ ...DESKTOP, mainMinimized: true })).toBe("float");
  });
});

describe("decisionSurface — browser build", () => {
  const WEB = { ...DESKTOP, webBuild: true };

  // A tab has no second window: `show_decision_float` answers null and swallows
  // the card. Every one of these used to resolve to "float", which is why a
  // pending fleet__ask on fleet-cloud rendered nowhere at all.
  it("renders inline even when the standalone window is preferred", () => {
    expect(decisionSurface({ ...WEB, floatingPreferred: true })).toBe("inline");
  });

  it("renders inline in Lite mode", () => {
    expect(decisionSurface({ ...WEB, liteMode: true })).toBe("inline");
  });

  it("renders inline when the host reports a minimized main window", () => {
    expect(decisionSurface({ ...WEB, mainMinimized: true })).toBe("inline");
  });

  it("renders inline with every float trigger set at once", () => {
    expect(
      decisionSurface({
        webBuild: true,
        floatingPreferred: true,
        liteMode: true,
        mainMinimized: true,
      }),
    ).toBe("inline");
  });
});
