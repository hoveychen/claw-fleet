// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  classifyScrollAttempt,
  installScrollFreezeProbe,
  isFaultVerdict,
  type ProbeContext,
  type ScrollAttempt,
} from "./scrollFreezeProbe";

const attempt = (over: Partial<ScrollAttempt> = {}): ScrollAttempt => ({
  overflow: 5000,
  room: 3000,
  moved: 0,
  intent: -300,
  pinFired: false,
  ...over,
});

describe("classifyScrollAttempt", () => {
  it("reports a freeze when there was room, input arrived, and nothing moved", () => {
    // The reported fault: viewport pinned at the bottom of a long transcript,
    // scrolling up does nothing at all, and the auto-follow pin never ran.
    expect(classifyScrollAttempt(attempt())).toBe("frozen");
  });

  it("calls it a yank when the pin wrote scrollTop during the gesture", () => {
    expect(classifyScrollAttempt(attempt({ moved: 0, pinFired: true }))).toBe("yanked");
    // Moved up, then got dragged back down to where it started.
    expect(classifyScrollAttempt(attempt({ moved: 1, pinFired: true }))).toBe("yanked");
  });

  it("reports no-overflow when the container thinks it has nothing to scroll", () => {
    expect(classifyScrollAttempt(attempt({ overflow: 0, room: 0 }))).toBe("no-overflow");
  });

  it("stays quiet when the gesture landed", () => {
    expect(classifyScrollAttempt(attempt({ moved: -120 }))).toBe("ok");
    expect(classifyScrollAttempt(attempt({ intent: 300, moved: 120 }))).toBe("ok");
  });

  it("stays quiet at the end stop, which is where auto-follow parks the reader", () => {
    // Sitting at the bottom and scrolling further down: nothing moves, and
    // that is correct. Without the room check this is the fault report that
    // would drown out every real one.
    expect(classifyScrollAttempt(attempt({ intent: 300, room: 0, moved: 0 }))).toBe("at-limit");
    // Same at the top, scrolling up.
    expect(classifyScrollAttempt(attempt({ intent: -300, room: 0, moved: 0 }))).toBe("at-limit");
  });

  it("treats sub-pixel movement as no movement", () => {
    expect(classifyScrollAttempt(attempt({ moved: -0.5 }))).toBe("frozen");
  });

  it("only flags the three fault verdicts", () => {
    expect(isFaultVerdict("frozen")).toBe(true);
    expect(isFaultVerdict("yanked")).toBe(true);
    expect(isFaultVerdict("no-overflow")).toBe(true);
    expect(isFaultVerdict("ok")).toBe(false);
    expect(isFaultVerdict("at-limit")).toBe(false);
  });
});

describe("installScrollFreezeProbe", () => {
  let el: HTMLDivElement;
  let lines: string[];
  let pins: number;
  let clock: number;
  let teardown: () => void;
  let revives: number;
  /** Whether the stand-in revive actually brings the scroll layer back. */
  let reviveReanimates = true;

  /** jsdom gives every element zero metrics; stand in for a real layout. */
  const sizeAs = (scrollHeight: number, clientHeight: number, scrollTop: number) => {
    Object.defineProperty(el, "scrollHeight", { value: scrollHeight, configurable: true });
    Object.defineProperty(el, "clientHeight", { value: clientHeight, configurable: true });
    el.scrollTop = scrollTop;
  };

  /** Stand-in for the content-height watcher SessionDetail owns. */
  let growth: { sinceMs: number; byPx: number } | null;

  const ctx = (): ProbeContext => ({
    sessionId: () => "sess-1",
    status: () => "waiting",
    messageCount: () => 420,
    isFollowing: () => true,
    pinCount: () => pins,
    contentGrowth: () => growth,
  });

  const wheel = (deltaY: number) => {
    const ev = new Event("wheel") as WheelEvent;
    Object.defineProperty(ev, "deltaY", { value: deltaY });
    el.dispatchEvent(ev);
  };

  /** rAF callbacks the probe queued, run on demand by `flushFrames`. */
  let frames: FrameRequestCallback[];

  /** Run every frame callback queued so far — one animation frame passing. */
  const flushFrames = () => {
    const due = frames;
    frames = [];
    for (const cb of due) cb(0);
  };

  beforeEach(() => {
    vi.useFakeTimers();
    frames = [];
    // Held rather than executed, so a test can say "one frame passed" without
    // also letting the 140ms gesture-end timer fire. Telling those two apart is
    // the whole point of the in-gesture tests below.
    vi.stubGlobal("requestAnimationFrame", (cb: FrameRequestCallback) => {
      frames.push(cb);
      return frames.length;
    });
    el = document.createElement("div");
    // jsdom's scrollTop is a plain property, so assignment sticks — which is
    // what lets a test model "the browser moved it" vs "nothing moved".
    document.body.appendChild(el);
    lines = [];
    pins = 0;
    clock = 1_000_000;
    revives = 0;
    growth = null;
    // Stand in for the real revive so tests decide whether the scroll layer
    // comes back: `reviveReanimates` false models a container that stays dead.
    teardown = installScrollFreezeProbe(el, ctx(), (l) => lines.push(l), () => clock, () => {
      revives += 1;
      return reviveReanimates;
    });
  });

  afterEach(() => {
    teardown();
    el.remove();
    reviveReanimates = true;
    vi.unstubAllGlobals();
    vi.useRealTimers();
  });

  describe("keeping up with the gesture instead of waiting for it to end", () => {
    // The repair used to run only from the gesture-end timer, so a frozen pane
    // stayed visually dead for the whole swipe and then jumped once the reader
    // stopped — reported as "视图没更新，只有滚动停止后就忽然间更新到新的位置".
    // Field line behind it (2026-08-19 14:47:49): intent=-1163 moved=0
    // room=15625, then revive=recovered landed=14462.

    it("moves the viewport one frame in, without the gesture ending", () => {
      sizeAs(9000, 800, 8200);
      wheel(-300);
      flushFrames();

      // No timer advance: the gesture is still going.
      expect(el.scrollTop).toBe(7900);
    });

    it("follows every later wheel event in the same gesture immediately", () => {
      sizeAs(9000, 800, 8200);
      wheel(-100);
      flushFrames(); // freeze detected here; the pane is under our control now
      wheel(-100);
      wheel(-100);

      // Each event lands as it arrives — no second frame, no gesture end.
      expect(el.scrollTop).toBe(7900);
    });

    it("does not double-apply when the browser's own scroll lands late", () => {
      // The danger of driving scrollTop by hand: if WebKit was merely reporting
      // a stale scrollTop and later applies the same wheels, a relative `+=`
      // would move the reader twice as far as they asked. Absolute targets make
      // that impossible.
      sizeAs(9000, 800, 8200);
      wheel(-100);
      flushFrames();
      expect(el.scrollTop).toBe(8100);

      el.scrollTop = 8000; // the browser catches up on its own
      wheel(-100); // reader has now asked for 200 total

      expect(el.scrollTop).toBe(8000); // 8200 - 200, not 7900
    });

    it("clamps the in-gesture position to the scrollable range", () => {
      sizeAs(9000, 800, 100);
      wheel(-4000);
      flushFrames();

      expect(el.scrollTop).toBe(0);
    });

    it("leaves a healthy pane to scroll itself", () => {
      sizeAs(9000, 800, 8200);
      el.addEventListener("wheel", () => {
        el.scrollTop = 7900; // the browser honours the wheel, as it should
      });
      wheel(-300);
      flushFrames();
      wheel(-300);

      expect(revives).toBe(0);
      // Untouched by us: 7900 from the listener above, not a driven 7600.
      expect(el.scrollTop).toBe(7900);
    });

    it("stays out of the way at the end stop", () => {
      sizeAs(9000, 800, 8200);
      wheel(300); // already parked at the bottom
      flushFrames();

      expect(revives).toBe(0);
      expect(el.scrollTop).toBe(8200);
    });

    it("still reports the freeze it took over, so the log keeps its evidence", () => {
      // Driving scrollTop ourselves makes `moved` look healthy by the time the
      // gesture ends. Without saying so explicitly, every taken-over freeze
      // would classify as "ok" and vanish from the log.
      sizeAs(9000, 800, 8200);
      wheel(-300);
      flushFrames();
      vi.advanceTimersByTime(200);

      expect(lines).toHaveLength(1);
      expect(lines[0]).toContain("verdict=frozen");
      expect(lines[0]).toContain("takeover=1");
      expect(lines[0]).toContain("nativeMoved=0");
    });

    it("re-tests each gesture rather than latching onto the pane forever", () => {
      // A pane that recovers must get its native scrolling back: a latched
      // takeover would keep overriding it, and one misjudgement would then
      // last for the rest of the session.
      sizeAs(9000, 800, 8200);
      wheel(-100);
      flushFrames();
      expect(revives).toBe(1);

      vi.advanceTimersByTime(200); // gesture over
      el.addEventListener("wheel", () => {
        el.scrollTop = el.scrollTop - 100; // healthy again
      });
      wheel(-100);
      flushFrames();

      expect(revives).toBe(1); // no second takeover
    });

    it("does not fight the auto-follow pin", () => {
      // A yank is our own bug. Driving scrollTop against the pin would just
      // start a tug of war on every frame.
      sizeAs(9000, 800, 8200);
      el.addEventListener("wheel", () => {
        el.scrollTop = 7900;
        el.scrollTop = 8200;
        pins += 1;
      });
      wheel(-300);
      flushFrames();

      expect(revives).toBe(0);
      expect(el.scrollTop).toBe(8200);
    });
  });

  it("logs a frozen verdict with the metrics needed to tell layout from compositor", () => {
    sizeAs(9000, 800, 8200); // pinned at the bottom, 8200px of history above
    wheel(-300);
    vi.advanceTimersByTime(200);

    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("verdict=frozen");
    expect(lines[0]).toContain("scrollTop=8200");
    expect(lines[0]).toContain("scrollHeight=9000");
    expect(lines[0]).toContain("clientHeight=800");
    expect(lines[0]).toContain("room=8200");
    expect(lines[0]).toContain("session=sess-1");
    expect(lines[0]).toContain("msgs=420");
  });

  it("says nothing when the gesture actually scrolled", () => {
    sizeAs(9000, 800, 8200);
    el.addEventListener("wheel", () => {
      el.scrollTop = 7900; // stand in for the browser honouring the wheel
    });
    wheel(-300);
    vi.advanceTimersByTime(200);

    expect(lines).toEqual([]);
  });

  it("says nothing when already parked against the end stop", () => {
    sizeAs(9000, 800, 8200);
    wheel(300); // scrolling further down while already at the bottom
    vi.advanceTimersByTime(200);

    expect(lines).toEqual([]);
  });

  it("blames the auto-follow pin when it wrote scrollTop mid-gesture", () => {
    sizeAs(9000, 800, 8200);
    el.addEventListener("wheel", () => {
      el.scrollTop = 7900;
      // The ResizeObserver pin fires and drags it back to the bottom.
      el.scrollTop = 8200;
      pins += 1;
    });
    wheel(-300);
    vi.advanceTimersByTime(200);

    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("verdict=yanked");
    expect(lines[0]).toContain("pins=1");
  });

  it("treats one continuous gesture as one attempt", () => {
    sizeAs(9000, 800, 8200);
    wheel(-100);
    vi.advanceTimersByTime(50);
    wheel(-100);
    vi.advanceTimersByTime(50);
    wheel(-100);
    vi.advanceTimersByTime(200);

    expect(lines).toHaveLength(1);
    expect(lines[0]).toContain("intent=-300");
  });

  it("rate-limits so a stuck pane cannot flood the log", () => {
    sizeAs(9000, 800, 8200);
    for (let i = 0; i < 5; i++) {
      wheel(-300);
      vi.advanceTimersByTime(200);
    }
    expect(lines).toHaveLength(1);

    clock += 31_000;
    wheel(-300);
    vi.advanceTimersByTime(200);
    expect(lines).toHaveLength(2);
  });

  it("revives the scroll layer when it finds one frozen", () => {
    sizeAs(9000, 800, 8200);
    wheel(-300);
    vi.advanceTimersByTime(200);

    expect(revives).toBe(1);
  });

  it("finishes the scroll the reader asked for, so the gesture isn't lost", () => {
    // Reviving alone would leave them staring at the same unmoved viewport and
    // having to scroll again. Apply the intent they already expressed.
    sizeAs(9000, 800, 8200);
    reviveReanimates = true;
    wheel(-300);
    vi.advanceTimersByTime(200);

    expect(el.scrollTop).toBe(7900); // 8200 - 300
  });

  it("clamps the recovered position to the scrollable range", () => {
    sizeAs(9000, 800, 100);
    wheel(-4000); // far past the top
    vi.advanceTimersByTime(200);

    expect(el.scrollTop).toBe(0);
  });

  it("records whether the revive actually worked", () => {
    // This is the self-check: it says in the log whether the fix fixed it,
    // rather than leaving the claim to rest on my say-so.
    sizeAs(9000, 800, 8200);
    wheel(-300);
    vi.advanceTimersByTime(200);

    expect(lines[0]).toContain("revive=recovered");
  });

  it("says so when the revive did not bring it back", () => {
    sizeAs(9000, 800, 8200);
    // A dead container: the stand-in reports it ran, but scrollTop is frozen
    // solid, so writing it changes nothing.
    Object.defineProperty(el, "scrollTop", {
      configurable: true,
      get: () => 8200,
      set: () => {},
    });
    wheel(-300);
    vi.advanceTimersByTime(200);

    expect(lines[0]).toContain("revive=failed");
  });

  it("leaves a healthy container alone", () => {
    sizeAs(9000, 800, 8200);
    el.addEventListener("wheel", () => {
      el.scrollTop = 7900;
    });
    wheel(-300);
    vi.advanceTimersByTime(200);

    expect(revives).toBe(0);
  });

  it("does not revive when the pin was the one holding it", () => {
    // A yank is a behaviour bug in our own code; rebuilding the scroll layer
    // would be treating a symptom that has nothing to do with the compositor.
    sizeAs(9000, 800, 8200);
    el.addEventListener("wheel", () => {
      el.scrollTop = 7900;
      el.scrollTop = 8200;
      pins += 1;
    });
    wheel(-300);
    vi.advanceTimersByTime(200);

    expect(revives).toBe(0);
  });

  it("keeps trying on later gestures even while the log is rate-limited", () => {
    // The log is throttled to one line per 30s so a stuck pane can't flood it,
    // but every gesture the reader makes still deserves a repair attempt.
    sizeAs(9000, 800, 8200);
    for (let i = 0; i < 4; i++) {
      sizeAs(9000, 800, 8200);
      wheel(-300);
      vi.advanceTimersByTime(200);
    }

    expect(lines).toHaveLength(1);
    expect(revives).toBe(4);
  });

  describe("content-growth timing, to test the WebKit #150974 hypothesis", () => {
    // WebKit bug 150974 — "dynamic content in a scrollable container breaks
    // scrolling" — would explain this freeze, and our prevention layer has the
    // matching gap: `installScrollBoxResizeRevive` watches the box's *own*
    // height, nothing watches the content's. Before acting on that, the log has
    // to say whether freezes actually follow content changes. No hypothesis
    // gets a fix until the field data backs it.

    it("reports how recently the content changed height, and by how much", () => {
      sizeAs(9000, 800, 8200);
      growth = { sinceMs: 180, byPx: 1240 };
      wheel(-300);
      vi.advanceTimersByTime(200);

      expect(lines[0]).toContain("sinceGrow=180");
      expect(lines[0]).toContain("growBy=1240");
    });

    it("says so when the content has not changed at all", () => {
      // An idle session whose transcript settled long ago still freezes — field
      // logs show status=idle freezes — so "no growth" has to be reportable and
      // distinguishable from "grew 0px just now", or the hypothesis can't fail.
      sizeAs(9000, 800, 8200);
      growth = null;
      wheel(-300);
      vi.advanceTimersByTime(200);

      expect(lines[0]).toContain("sinceGrow=none");
      expect(lines[0]).toContain("growBy=0");
    });
  });

  it("stops listening after teardown", () => {
    sizeAs(9000, 800, 8200);
    teardown();
    wheel(-300);
    vi.advanceTimersByTime(200);
    expect(lines).toEqual([]);
  });
});
