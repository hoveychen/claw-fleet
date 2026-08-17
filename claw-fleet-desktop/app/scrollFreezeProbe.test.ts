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

  /** jsdom gives every element zero metrics; stand in for a real layout. */
  const sizeAs = (scrollHeight: number, clientHeight: number, scrollTop: number) => {
    Object.defineProperty(el, "scrollHeight", { value: scrollHeight, configurable: true });
    Object.defineProperty(el, "clientHeight", { value: clientHeight, configurable: true });
    el.scrollTop = scrollTop;
  };

  const ctx = (): ProbeContext => ({
    sessionId: () => "sess-1",
    status: () => "waiting",
    messageCount: () => 420,
    isFollowing: () => true,
    pinCount: () => pins,
  });

  const wheel = (deltaY: number) => {
    const ev = new Event("wheel") as WheelEvent;
    Object.defineProperty(ev, "deltaY", { value: deltaY });
    el.dispatchEvent(ev);
  };

  beforeEach(() => {
    vi.useFakeTimers();
    el = document.createElement("div");
    // jsdom's scrollTop is a plain property, so assignment sticks — which is
    // what lets a test model "the browser moved it" vs "nothing moved".
    document.body.appendChild(el);
    lines = [];
    pins = 0;
    clock = 1_000_000;
    teardown = installScrollFreezeProbe(el, ctx(), (l) => lines.push(l), () => clock);
  });

  afterEach(() => {
    teardown();
    el.remove();
    vi.useRealTimers();
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

  it("stops listening after teardown", () => {
    sizeAs(9000, 800, 8200);
    teardown();
    wheel(-300);
    vi.advanceTimersByTime(200);
    expect(lines).toEqual([]);
  });
});
