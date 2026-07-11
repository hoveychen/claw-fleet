import { describe, it, expect } from "vitest";
import {
  FRAME_MAX_HEIGHT,
  FRAME_MIN_HEIGHT,
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
