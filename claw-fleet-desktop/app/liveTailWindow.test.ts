import { describe, expect, it } from "vitest";

import { LIVE_TAIL_FLOOR, nextLiveTail } from "./liveTailWindow";

/**
 * Replay the poll loop the way `SessionDetail` runs it: every tick asks for the
 * current window, the transcript hands back `min(window, fileLength)` records,
 * and the rule picks the next window.
 *
 * `arrived` is how many records the agent appended between ticks — 0 on an
 * idle session, a handful on a busy one.
 */
function replayPolls({
  fileLength,
  ticks,
  arrivedPerTick = 0,
}: {
  fileLength: number;
  ticks: number;
  arrivedPerTick?: number;
}): number[] {
  let tail = LIVE_TAIL_FLOOR;
  let length = fileLength;
  const windows: number[] = [];
  for (let i = 0; i < ticks; i++) {
    const returned = Math.min(tail, length);
    tail = nextLiveTail({ tail, returned, arrived: arrivedPerTick });
    windows.push(tail);
    length += arrivedPerTick;
  }
  return windows;
}

describe("nextLiveTail", () => {
  it("leaves the window alone while the transcript is shorter than it", () => {
    expect(nextLiveTail({ tail: 150, returned: 42, arrived: 0 })).toBe(150);
    expect(nextLiveTail({ tail: 150, returned: 149, arrived: 3 })).toBe(150);
  });

  it("does not escalate on an idle session whose transcript exceeds the window", () => {
    // The regression. A 4513-record session polled every 1.5s used to climb
    // 150 → 1150 → 2150 → 3150 → 4150 → 5150 and then re-read the whole file on
    // every tick — measured in claw-fleet-debug.log at 2026-09-06 18:22, where
    // each poll returned 4513 records and took 1–3s against a 1.5s interval.
    // Nothing was appended in that window, so nothing justified growing it.
    const windows = replayPolls({ fileLength: 4513, ticks: 6 });
    expect(windows).toEqual([150, 150, 150, 150, 150, 150]);
  });

  it("grows by exactly what arrived, so the window's start stays pinned", () => {
    expect(nextLiveTail({ tail: 150, returned: 150, arrived: 3 })).toBe(153);
    expect(nextLiveTail({ tail: 153, returned: 153, arrived: 0 })).toBe(153);
  });

  it("caps the window however long the session runs", () => {
    const windows = replayPolls({ fileLength: 5000, ticks: 400, arrivedPerTick: 5 });
    expect(Math.max(...windows)).toBeLessThanOrEqual(1200);
  });
});
