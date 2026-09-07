import { describe, it, expect } from "vitest";
import {
  createQuietLatch,
  stickyQuiet,
  DENSE_WRITE_MS,
} from "../../shared-ts/quietLatch";

/** The bug this file pins: the run-status dot alternated between solid green
 *  ("agent working") and faded green ("alive but quiet") on the same session,
 *  several times a minute. Core's `determine_status` holds a live status for a
 *  hard window only (30s idle fallthrough, 60s tool_use, 120s trailing user
 *  message), so a session that writes its transcript every few minutes — one
 *  parked on a long build, say — flips live → decayed → live → decayed. The
 *  latch below makes the faded state sticky: a lone sparse write no longer
 *  wins the dot back to solid green; two writes close together do. */
describe("quiet-alive latch (anti-flicker hysteresis)", () => {
  const t0 = 1_000_000;

  it("a session that was never quiet is not latched", () => {
    const st = createQuietLatch();
    expect(stickyQuiet(st, "s1", { alive: true, rawQuiet: false, lastActivityMs: t0, now: t0 })).toBe(false);
  });

  it("raw quiet latches immediately", () => {
    const st = createQuietLatch();
    expect(
      stickyQuiet(st, "s1", { alive: true, rawQuiet: true, lastActivityMs: t0 - 60_000, now: t0 }),
    ).toBe(true);
  });

  it("one sparse write does NOT unlatch — this is the flicker", () => {
    const st = createQuietLatch();
    stickyQuiet(st, "s1", { alive: true, rawQuiet: true, lastActivityMs: t0, now: t0 });
    // Three minutes later the session writes one line; core flips the status
    // back to a live one for its hard window. The dot must stay faded.
    const write = t0 + 180_000;
    expect(stickyQuiet(st, "s1", { alive: true, rawQuiet: false, lastActivityMs: write, now: write })).toBe(
      true,
    );
    // …and it must still be faded once that window decays back to quiet.
    expect(
      stickyQuiet(st, "s1", { alive: true, rawQuiet: true, lastActivityMs: write, now: write + 40_000 }),
    ).toBe(true);
  });

  it("two writes close together unlatch — the session is genuinely working again", () => {
    const st = createQuietLatch();
    stickyQuiet(st, "s1", { alive: true, rawQuiet: true, lastActivityMs: t0, now: t0 });
    const first = t0 + 180_000;
    stickyQuiet(st, "s1", { alive: true, rawQuiet: false, lastActivityMs: first, now: first });
    const second = first + DENSE_WRITE_MS - 1_000;
    expect(stickyQuiet(st, "s1", { alive: true, rawQuiet: false, lastActivityMs: second, now: second })).toBe(
      false,
    );
    // Once unlatched it stays unlatched while the writes keep coming.
    const third = second + 5_000;
    expect(stickyQuiet(st, "s1", { alive: true, rawQuiet: false, lastActivityMs: third, now: third })).toBe(
      false,
    );
  });

  it("sparse writes keep re-arming the latch instead of accumulating into a recovery", () => {
    const st = createQuietLatch();
    stickyQuiet(st, "s1", { alive: true, rawQuiet: true, lastActivityMs: t0, now: t0 });
    let at = t0;
    for (let i = 0; i < 5; i++) {
      at += 120_000; // one line every two minutes: sparse, never dense
      expect(stickyQuiet(st, "s1", { alive: true, rawQuiet: false, lastActivityMs: at, now: at })).toBe(true);
    }
  });

  it("repeated observations of the same timestamp are idempotent", () => {
    const st = createQuietLatch();
    stickyQuiet(st, "s1", { alive: true, rawQuiet: true, lastActivityMs: t0, now: t0 });
    const write = t0 + 180_000;
    // The list re-renders many times per poll; observing the same activity
    // timestamp again must not read as a second (dense) write.
    for (let i = 0; i < 4; i++) {
      expect(
        stickyQuiet(st, "s1", { alive: true, rawQuiet: false, lastActivityMs: write, now: write + i * 1_000 }),
      ).toBe(true);
    }
  });

  it("a dead process drops the latch outright", () => {
    // Otherwise a genuinely ended session would keep wearing the faded dot,
    // and a later resume reusing that id would open on a dimmed row.
    const st = createQuietLatch();
    stickyQuiet(st, "s1", { alive: true, rawQuiet: true, lastActivityMs: t0, now: t0 });
    expect(
      stickyQuiet(st, "s1", { alive: false, rawQuiet: false, lastActivityMs: t0, now: t0 + 1_000 }),
    ).toBe(false);
    expect(st.size).toBe(0);
  });

  it("latches are per session", () => {
    const st = createQuietLatch();
    stickyQuiet(st, "s1", { alive: true, rawQuiet: true, lastActivityMs: t0, now: t0 });
    expect(stickyQuiet(st, "s2", { alive: true, rawQuiet: false, lastActivityMs: t0, now: t0 })).toBe(false);
  });
});
