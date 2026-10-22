// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { fitTextareaHeight } from "./ChatComposer";

// jsdom has no layout, so the scroll container is modelled by hand: its
// maximum scrollTop shrinks while the textarea is collapsed to 0px, the way a
// real browser clamps it, and scrollHeight is a fixed content height.
function setup(initialTop: number) {
  const scroller = document.createElement("div");
  const ta = document.createElement("textarea");
  scroller.appendChild(ta);
  document.body.appendChild(scroller);

  const contentHeight = 180;
  Object.defineProperty(ta, "scrollHeight", { get: () => contentHeight });
  let top = initialTop;
  const maxTop = () => (ta.style.height === "0px" ? initialTop - contentHeight : 10_000);
  Object.defineProperty(scroller, "scrollTop", {
    get: () => top,
    set: (v: number) => {
      top = Math.max(0, Math.min(v, maxTop()));
    },
  });
  // A real browser clamps as soon as the box shrinks; emulate that on read.
  const clampOnCollapse = () => {
    top = Math.max(0, Math.min(top, maxTop()));
  };
  return { scroller, ta, contentHeight, clampOnCollapse, getTop: () => top };
}

describe("fitTextareaHeight", () => {
  it("sizes the textarea to its content", () => {
    const { ta, contentHeight } = setup(0);
    fitTextareaHeight(ta);
    expect(ta.style.height).toBe(`${contentHeight}px`);
  });

  it("restores an ancestor's scrollTop clamped by the transient collapse", () => {
    const { scroller, ta, clampOnCollapse, getTop } = setup(600);
    // Trigger the clamp at the moment the height becomes 0px.
    const style = ta.style;
    let h = "";
    Object.defineProperty(style, "height", {
      get: () => h,
      set: (v: string) => {
        h = v;
        if (v === "0px") clampOnCollapse();
      },
    });
    fitTextareaHeight(ta);
    expect(getTop()).toBe(600);
    expect(scroller.scrollTop).toBe(600);
  });
});
