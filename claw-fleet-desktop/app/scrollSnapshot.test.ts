// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";

import {
  formatSnapshot,
  probeScrollWrite,
  takeScrollSnapshot,
  type ViewMetrics,
} from "./scrollSnapshot";

const VIEW: ViewMetrics = {
  innerWidth: 1440,
  innerHeight: 900,
  documentClientHeight: 900,
  visualViewportHeight: 900,
};

/**
 * jsdom has no layout, so `scrollHeight`/`clientHeight` are 0 and `scrollTop`
 * is a plain stored value that always accepts a write. Both behaviours are
 * overridden per element here — the point of these tests is the two readings
 * the probe has to tell apart, and only a box that refuses the write can stand
 * in for a stuck scroller.
 */
function makeScroller({
  scrollHeight,
  clientHeight,
  movable,
  startAt = 0,
}: {
  scrollHeight: number;
  clientHeight: number;
  movable: boolean;
  startAt?: number;
}): HTMLDivElement {
  const el = document.createElement("div");
  document.body.appendChild(el);
  let top = startAt;
  Object.defineProperty(el, "scrollHeight", { get: () => scrollHeight });
  Object.defineProperty(el, "clientHeight", { get: () => clientHeight });
  Object.defineProperty(el, "offsetHeight", { get: () => clientHeight });
  Object.defineProperty(el, "scrollTop", {
    get: () => top,
    set: (v: number) => {
      if (!movable) return;
      top = Math.max(0, Math.min(v, scrollHeight - clientHeight));
    },
  });
  return el;
}

afterEach(() => {
  document.body.innerHTML = "";
});

describe("probeScrollWrite", () => {
  it("reports the distance moved and puts the position back", () => {
    const el = makeScroller({ scrollHeight: 5000, clientHeight: 600, movable: true, startAt: 900 });
    expect(probeScrollWrite(el)).toBe(-120);
    expect(el.scrollTop).toBe(900);
  });

  it("pushes downward when the box sits at the top", () => {
    const el = makeScroller({ scrollHeight: 5000, clientHeight: 600, movable: true });
    expect(probeScrollWrite(el)).toBe(120);
    expect(el.scrollTop).toBe(0);
  });

  it("reads 0 on a box with room that refuses to move", () => {
    const el = makeScroller({ scrollHeight: 5000, clientHeight: 600, movable: false });
    expect(probeScrollWrite(el)).toBe(0);
  });

  it("reads 0 on a box that has nothing to scroll", () => {
    const el = makeScroller({ scrollHeight: 600, clientHeight: 600, movable: true });
    expect(probeScrollWrite(el)).toBe(0);
  });
});

describe("takeScrollSnapshot", () => {
  it("records the geometry that separates 'no room' from 'room but stuck'", () => {
    const stuck = makeScroller({ scrollHeight: 5000, clientHeight: 600, movable: false });
    const empty = makeScroller({ scrollHeight: 600, clientHeight: 600, movable: true });

    const a = takeScrollSnapshot(stuck, VIEW, "stuck");
    const b = takeScrollSnapshot(empty, VIEW, "empty");

    // Both refuse the write; only the heights tell them apart.
    expect(a.writeTest).toBe(0);
    expect(b.writeTest).toBe(0);
    expect(a.scrollHeight - a.clientHeight).toBe(4400);
    expect(b.scrollHeight - b.clientHeight).toBe(0);
  });

  it("walks the ancestor chain and each direct child", () => {
    const el = makeScroller({ scrollHeight: 5000, clientHeight: 600, movable: true });
    const outer = document.createElement("div");
    outer.className = "pane";
    document.body.appendChild(outer);
    outer.appendChild(el);
    el.appendChild(document.createElement("div"));
    el.appendChild(document.createElement("div"));

    const snap = takeScrollSnapshot(el, VIEW, "live");
    expect(snap.childHeights).toHaveLength(2);
    expect(snap.boxes.map((b) => b.className)).toContain("pane");
  });
});

describe("takeScrollSnapshot — DOM row counts", () => {
  /** A scroller holding a message list with `rows` rendered rows. */
  function withList(el: HTMLDivElement, rows: number) {
    const list = document.createElement("div");
    el.appendChild(list);
    for (let i = 0; i < rows; i++) {
      const row = document.createElement("div");
      row.setAttribute("data-msg-idx", String(i));
      list.appendChild(row);
    }
  }

  it("counts the rows the DOM actually holds", () => {
    const el = makeScroller({ scrollHeight: 5000, clientHeight: 600, movable: true });
    withList(el, 37);
    const snap = takeScrollSnapshot(el, VIEW, "live");
    expect(snap.dom.msgIdxNodes).toBe(37);
    expect(snap.dom.listChildren).toBe(37);
  });

  it("reads zero on a scroller with no list at all", () => {
    const el = makeScroller({ scrollHeight: 600, clientHeight: 600, movable: true });
    const snap = takeScrollSnapshot(el, VIEW, "empty");
    expect(snap.dom).toEqual({ msgIdxNodes: 0, listChildren: 0 });
  });

  it("puts the DOM count and the owner's count side by side", () => {
    // The whole reason the counts were added: a pane measured holding one
    // message while its owner had 1621 renderable records in hand. Neither
    // half is remarkable alone; together they are the contradiction.
    const el = makeScroller({ scrollHeight: 761, clientHeight: 623, movable: true });
    withList(el, 1);
    const text = formatSnapshot(
      takeScrollSnapshot(el, VIEW, "stuck", { msgs: 4513, renderable: 1621, tail: 5150 }),
    );
    expect(text).toContain("dom msgIdxNodes=1");
    expect(text).toContain("state msgs=4513 renderable=1621 tail=5150");
  });
});

describe("formatSnapshot", () => {
  it("puts every number on a pasteable line", () => {
    const el = makeScroller({ scrollHeight: 5000, clientHeight: 600, movable: true });
    const text = formatSnapshot(takeScrollSnapshot(el, VIEW, "frozen"));
    expect(text).toContain("[frozen]");
    expect(text).toContain("scrollHeight=5000");
    expect(text).toContain("clientHeight=600");
    expect(text).toContain("writeTest=120");
    expect(text).toContain("innerH=900");
  });

  it("omits the state line when the caller passed no counts", () => {
    const el = makeScroller({ scrollHeight: 5000, clientHeight: 600, movable: true });
    expect(formatSnapshot(takeScrollSnapshot(el, VIEW, "bare"))).not.toContain("state ");
  });
});
