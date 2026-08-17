/**
 * Whether the conversation pane is still following the newest message.
 *
 * Split out of `SessionDetail` so the rules can be exercised directly, the way
 * `messageWindow` splits out the render window.
 *
 * Following used to be decided by distance alone: within `FOLLOW_SLACK_PX` of
 * the bottom meant "following", and the auto-follow pin re-pinned on every
 * content growth. That reads the reader's *position* as their *intent*, and the
 * two disagree exactly when it matters — nudging up a little to re-read the
 * last answer leaves them inside the slack window, so the next growth drags
 * them back to the bottom. On a live session, where a poll rewrites the
 * transcript every 1.5s, that is a viewport that cannot be scrolled up at all.
 *
 * So a deliberate upward gesture detaches, and only returning to the bottom
 * re-attaches. Distance still decides everything else, which is what keeps a
 * streaming turn pinned without the reader touching anything.
 */

/** Distance from the bottom under which a scroll counts as "at the bottom". */
export const FOLLOW_SLACK_PX = 200;

/**
 * How close to the bottom the reader must come back to before following
 * resumes. Tighter than the slack window on purpose: having *asked* to stop
 * following, they should not be re-captured while still reading 150px up.
 */
export const REATTACH_SLACK_PX = 24;

export interface FollowState {
  /** Pin the viewport to the newest message on every content growth. */
  following: boolean;
  /** The reader scrolled up on purpose and has not come back yet. */
  detached: boolean;
}

export type FollowInput =
  /** The reader worked the wheel/trackpad. Negative intent is upward. */
  | { kind: "gesture"; intent: number }
  /** The container scrolled, for any reason. */
  | { kind: "scroll"; distFromBottom: number };

export const initialFollowState: FollowState = { following: true, detached: false };

export function nextFollowState(current: FollowState, input: FollowInput): FollowState {
  if (input.kind === "gesture") {
    // Upward is the one unambiguous "stop following" signal. Downward says
    // nothing on its own — the scroll it produces is judged on distance, so
    // running into the bottom re-attaches and stopping short does not.
    if (input.intent < 0) return { following: false, detached: true };
    return current;
  }

  if (current.detached) {
    const back = input.distFromBottom <= REATTACH_SLACK_PX;
    return { following: back, detached: !back };
  }
  return { following: input.distFromBottom < FOLLOW_SLACK_PX, detached: false };
}
