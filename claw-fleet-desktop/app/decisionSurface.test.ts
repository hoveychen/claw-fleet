import { describe, it, expect } from "vitest";
import { decisionSurfaces } from "./decisionSurface";

const DESKTOP = {
  webBuild: false,
  floatingPreferred: false,
  liteMode: false,
  mainMinimized: false,
};

describe("decisionSurfaces — desktop", () => {
  it("renders inline and pops nothing by default", () => {
    expect(decisionSurfaces(DESKTOP)).toEqual({ inline: true, float: false });
  });

  it("hands the card to the standalone window when the user asked for it", () => {
    expect(decisionSurfaces({ ...DESKTOP, floatingPreferred: true })).toEqual({
      inline: false,
      float: true,
    });
  });

  it("pops the float in Lite mode, which draws no in-window card", () => {
    expect(decisionSurfaces({ ...DESKTOP, liteMode: true })).toEqual({
      inline: false,
      float: true,
    });
  });

  // Both at once: the float is what the user can see, and the inline panel
  // stays mounted underneath so restoring the window does not lose its state.
  it("keeps the inline panel mounted while the main window is minimized", () => {
    expect(decisionSurfaces({ ...DESKTOP, mainMinimized: true })).toEqual({
      inline: true,
      float: true,
    });
  });
});

describe("decisionSurfaces — browser build", () => {
  const WEB = { ...DESKTOP, webBuild: true };

  // A tab has no second window: `show_decision_float` answers null and swallows
  // the card. Each of these used to resolve to float-only, which is why a
  // pending fleet__ask on fleet-cloud rendered nowhere at all.
  it("renders inline even when the standalone window is preferred", () => {
    expect(decisionSurfaces({ ...WEB, floatingPreferred: true })).toEqual({
      inline: true,
      float: false,
    });
  });

  it("renders inline in Lite mode", () => {
    expect(decisionSurfaces({ ...WEB, liteMode: true })).toEqual({
      inline: true,
      float: false,
    });
  });

  it("never asks for a float window, whatever the host reports", () => {
    expect(decisionSurfaces({ ...WEB, mainMinimized: true }).float).toBe(false);
  });

  it("renders inline with every float trigger set at once", () => {
    expect(
      decisionSurfaces({
        webBuild: true,
        floatingPreferred: true,
        liteMode: true,
        mainMinimized: true,
      }),
    ).toEqual({ inline: true, float: false });
  });
});
