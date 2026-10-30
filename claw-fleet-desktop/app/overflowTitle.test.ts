// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { isClampedTruncated, refreshOverflowTitle } from "./overflowTitle";

const base = {
  textOverflow: "clip",
  lineClamp: "none",
  scrollWidth: 100,
  clientWidth: 100,
  scrollHeight: 20,
  clientHeight: 20,
};

describe("isClampedTruncated", () => {
  it("is true for an ellipsis box whose content is wider than the box", () => {
    expect(isClampedTruncated({ ...base, textOverflow: "ellipsis", scrollWidth: 140 })).toBe(true);
  });
  it("is false for an ellipsis box whose content fits", () => {
    expect(isClampedTruncated({ ...base, textOverflow: "ellipsis" })).toBe(false);
  });
  it("is true for a line-clamped box taller than its clamp", () => {
    expect(isClampedTruncated({ ...base, lineClamp: "2", scrollHeight: 60 })).toBe(true);
  });
  it("ignores overflow without ellipsis or clamp", () => {
    expect(isClampedTruncated({ ...base, scrollWidth: 300, scrollHeight: 300 })).toBe(false);
  });
});

/** jsdom has no layout, so fake the sizes the browser would report. */
function box(text: string, scrollWidth: number, clientWidth: number): HTMLElement {
  const el = document.createElement("span");
  el.style.textOverflow = "ellipsis";
  el.textContent = text;
  Object.defineProperty(el, "scrollWidth", { configurable: true, get: () => scrollWidth });
  Object.defineProperty(el, "clientWidth", { configurable: true, get: () => clientWidth });
  document.body.appendChild(el);
  return el;
}

describe("refreshOverflowTitle", () => {
  it("adds the full text as a title when truncated, from a nested hover target", () => {
    const el = box("", 200, 80);
    const inner = document.createElement("b");
    inner.textContent = "P26f   relay 2: a long subtask title";
    el.appendChild(inner);
    refreshOverflowTitle(inner);
    expect(el.getAttribute("title")).toBe("P26f relay 2: a long subtask title");
  });

  it("leaves no title when the text fits, and drops a stale auto title", () => {
    let width = 200;
    const el = box("short", 0, 0);
    Object.defineProperty(el, "scrollWidth", { configurable: true, get: () => width });
    Object.defineProperty(el, "clientWidth", { configurable: true, get: () => 80 });
    refreshOverflowTitle(el);
    expect(el.getAttribute("title")).toBe("short");
    width = 60;
    refreshOverflowTitle(el);
    expect(el.hasAttribute("title")).toBe(false);
  });

  it("never overwrites an author-written title", () => {
    const el = box("long text", 200, 80);
    el.setAttribute("title", "custom");
    refreshOverflowTitle(el);
    expect(el.getAttribute("title")).toBe("custom");
  });

  it("does not add a title above a nearer element that already has one", () => {
    const el = box("", 200, 80);
    const inner = document.createElement("span");
    inner.setAttribute("title", "inner");
    inner.textContent = "long text";
    el.appendChild(inner);
    refreshOverflowTitle(inner);
    expect(el.hasAttribute("title")).toBe(false);
  });
});
