import { describe, expect, it } from "vitest";

import {
  DECISION_ANCHOR_MARGIN_PX,
  decisionAnchorTop,
  FOLLOW_SLACK_PX,
  initialFollowState,
  nextFollowState,
  type FollowState,
} from "./followState";

const following: FollowState = { following: true };
const detached = nextFollowState(following, { kind: "gesture", intent: -120 });

describe("nextFollowState", () => {
  it("starts pinned to the newest message", () => {
    expect(initialFollowState.following).toBe(true);
  });

  it("lets go the moment the reader scrolls up, however slightly", () => {
    // The bug this encodes: the reader nudges up to re-read the last answer
    // while sitting inside the slack window, so the distance rule still says
    // "following", and the auto-follow pin drags them back on the next content
    // growth. A deliberate upward gesture is the reader saying "stop
    // following" — it does not need to clear a 200px bar to be heard.
    expect(detached.following).toBe(false);
  });

  it("stays let go while the reader reads back, even inside the slack window", () => {
    // The scroll event that follows the gesture must not undo it: the reader
    // is 30px from the bottom, well within slack, and must stay detached.
    const after = nextFollowState(detached, { kind: "scroll", distFromBottom: 30 });
    expect(after.following).toBe(false);
  });

  it("resumes following once the reader returns to the bottom", () => {
    const back = nextFollowState(detached, { kind: "scroll", distFromBottom: 0 });
    expect(back.following).toBe(true);
  });

  it("ignores downward gestures, leaving the distance rule to decide", () => {
    const down = nextFollowState(detached, { kind: "gesture", intent: 120 });
    expect(down.following).toBe(false);
    expect(nextFollowState(down, { kind: "scroll", distFromBottom: 0 }).following).toBe(true);
  });

  it("follows programmatic scrolling that lands near the bottom", () => {
    // No gesture involved: growth that leaves the viewport inside the slack
    // window keeps following, so a streaming turn stays pinned.
    const near = nextFollowState(following, { kind: "scroll", distFromBottom: FOLLOW_SLACK_PX - 1 });
    expect(near.following).toBe(true);
  });

  it("drops following when content scrolls far from the bottom", () => {
    const far = nextFollowState(following, { kind: "scroll", distFromBottom: FOLLOW_SLACK_PX + 1 });
    expect(far.following).toBe(false);
  });
});

describe("decisionAnchorTop", () => {
  it("leaves a card that fits the visible area to the bottom pin", () => {
    expect(decisionAnchorTop(2000, 300, 600)).toBeNull();
  });

  it("puts the head of a card taller than the viewport at the top", () => {
    // The bug this encodes: a long card pinned to the bottom shows only its
    // options, and the reader has to scroll up to find what it is asking.
    expect(decisionAnchorTop(2000, 900, 600)).toBe(2000 - DECISION_ANCHOR_MARGIN_PX);
  });

  it("never asks for a negative scroll position", () => {
    expect(decisionAnchorTop(4, 900, 600)).toBe(0);
  });
});
