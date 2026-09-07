import { describe, it, expect } from "vitest";
import {
  FRAME_MAX_HEIGHT,
  FRAME_MIN_HEIGHT,
  framePreviewSrcDoc,
  parseFrameHeight,
  shouldApplyFrameHeight,
} from "./decisionFrame";

describe("parseFrameHeight", () => {
  it("accepts a positive height and rounds up", () => {
    expect(parseFrameHeight({ __fleetAskHeight: 342.2 })).toBe(343);
  });

  it("clamps to the card's usable band", () => {
    // A hostile card must not be able to shove the option buttons off-screen…
    expect(parseFrameHeight({ __fleetAskHeight: 99999 })).toBe(FRAME_MAX_HEIGHT);
    // …nor collapse itself into an unreadable sliver.
    expect(parseFrameHeight({ __fleetAskHeight: 3 })).toBe(FRAME_MIN_HEIGHT);
  });

  it("rejects anything that isn't a positive finite number", () => {
    for (const bad of [
      null,
      undefined,
      42,
      "300",
      {},
      { __fleetAskHeight: "300" },
      { __fleetAskHeight: 0 },
      { __fleetAskHeight: -10 },
      { __fleetAskHeight: NaN },
      { __fleetAskHeight: Infinity },
      { height: 300 },
    ]) {
      expect(parseFrameHeight(bad)).toBeNull();
    }
  });
});

describe("shouldApplyFrameHeight", () => {
  it("always applies the first measurement", () => {
    expect(shouldApplyFrameHeight(null, FRAME_MIN_HEIGHT)).toBe(true);
  });

  it("ignores churn inside the dead-band but follows real growth", () => {
    expect(shouldApplyFrameHeight(300, 301)).toBe(false);
    expect(shouldApplyFrameHeight(300, 298)).toBe(false);
    expect(shouldApplyFrameHeight(300, 420)).toBe(true);
    expect(shouldApplyFrameHeight(420, 300)).toBe(true);
  });
});

describe("framePreviewSrcDoc", () => {
  // The card the bug was found on: the agent styled its body with a light
  // foreground and left the background transparent, expecting the dark card
  // underneath to show through.
  const darkAssuming =
    "<style>body{color:#e6e6e6;background:transparent}</style><table><tr><td>x</td></tr></table>";

  it("declares the host theme's color-scheme so UA defaults match the card", () => {
    expect(framePreviewSrcDoc("<p>hi</p>", "dark")).toContain("color-scheme:dark");
    expect(framePreviewSrcDoc("<p>hi</p>", "light")).toContain("color-scheme:light");
  });

  it("gives an unstyled document a readable foreground per theme", () => {
    expect(framePreviewSrcDoc("<p>hi</p>", "dark")).toContain("#f7f8f8");
    expect(framePreviewSrcDoc("<p>hi</p>", "light")).toContain("#1f2023");
  });

  it("keeps the prelude ahead of the agent's own styles so the agent still wins", () => {
    const doc = framePreviewSrcDoc(darkAssuming, "dark");
    expect(doc.indexOf("color-scheme:dark")).toBeLessThan(doc.indexOf("#e6e6e6"));
    expect(doc).toContain(darkAssuming);
  });

  it("never paints an opaque background of its own", () => {
    // The card's own themed surface has to show through — an opaque白 here is
    // exactly what made the light-on-transparent table unreadable.
    for (const theme of ["dark", "light"] as const) {
      expect(framePreviewSrcDoc("<p>hi</p>", theme)).not.toMatch(/background:\s*#fff/);
    }
  });
});
