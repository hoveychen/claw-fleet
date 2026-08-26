// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";

import { nestedScrollerWillConsume } from "./nestedScroll";

describe("nestedScrollerWillConsume", () => {
  let pane: HTMLDivElement;

  const makePane = () => {
    pane = document.createElement("div");
    pane.style.overflowY = "auto";
    document.body.appendChild(pane);
    return pane;
  };

  /** jsdom gives every element zero metrics; stand in for a real layout. */
  const scroller = (
    parent: HTMLElement,
    m: { scrollHeight: number; clientHeight: number; scrollTop: number },
    overflowY = "auto",
  ) => {
    const box = document.createElement("div");
    box.style.overflowY = overflowY;
    Object.defineProperty(box, "scrollHeight", { value: m.scrollHeight, configurable: true });
    Object.defineProperty(box, "clientHeight", { value: m.clientHeight, configurable: true });
    box.scrollTop = m.scrollTop;
    parent.appendChild(box);
    return box;
  };

  const wheelAt = (target: HTMLElement, deltaY: number) => {
    const ev = new Event("wheel", { bubbles: true }) as WheelEvent;
    Object.defineProperty(ev, "deltaY", { value: deltaY });
    Object.defineProperty(ev, "target", { value: target });
    return ev;
  };

  afterEach(() => pane.remove());

  it("claims a gesture a card between the target and the pane can absorb", () => {
    const card = scroller(makePane(), { scrollHeight: 3000, clientHeight: 420, scrollTop: 900 });
    const text = document.createElement("p");
    card.appendChild(text);

    // Aimed at the paragraph deep inside the card, not the card itself.
    expect(nestedScrollerWillConsume(pane, wheelAt(text, -120))).toBe(true);
    expect(nestedScrollerWillConsume(pane, wheelAt(text, 120))).toBe(true);
  });

  it("hands the gesture back once the card has run out in that direction", () => {
    const atTop = scroller(makePane(), { scrollHeight: 3000, clientHeight: 420, scrollTop: 0 });

    expect(nestedScrollerWillConsume(pane, wheelAt(atTop, -120))).toBe(false);
    expect(nestedScrollerWillConsume(pane, wheelAt(atTop, 120))).toBe(true);

    const atBottom = scroller(makePane(), {
      scrollHeight: 3000,
      clientHeight: 420,
      scrollTop: 2580,
    });
    expect(nestedScrollerWillConsume(pane, wheelAt(atBottom, 120))).toBe(false);
    expect(nestedScrollerWillConsume(pane, wheelAt(atBottom, -120))).toBe(true);
  });

  it("ignores boxes that don't scroll", () => {
    // A `visible`/`hidden` box never absorbs a wheel, however tall its content.
    const clipped = scroller(
      makePane(),
      { scrollHeight: 3000, clientHeight: 420, scrollTop: 0 },
      "hidden",
    );
    expect(nestedScrollerWillConsume(pane, wheelAt(clipped, 120))).toBe(false);

    // An `auto` box whose content fits has nothing to give either.
    const short = scroller(makePane(), { scrollHeight: 420, clientHeight: 420, scrollTop: 0 });
    expect(nestedScrollerWillConsume(pane, wheelAt(short, 120))).toBe(false);
  });

  it("never claims the pane's own gesture", () => {
    makePane();
    Object.defineProperty(pane, "scrollHeight", { value: 9000, configurable: true });
    Object.defineProperty(pane, "clientHeight", { value: 500, configurable: true });
    pane.scrollTop = 4000;

    // The walk stops *at* the pane, so the pane's own overflow never counts.
    expect(nestedScrollerWillConsume(pane, wheelAt(pane, -120))).toBe(false);
  });

  it("says no for a wheel with no vertical component", () => {
    const card = scroller(makePane(), { scrollHeight: 3000, clientHeight: 420, scrollTop: 900 });
    expect(nestedScrollerWillConsume(pane, wheelAt(card, 0))).toBe(false);
  });
});
