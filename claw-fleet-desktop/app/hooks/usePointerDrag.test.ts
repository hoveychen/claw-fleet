// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createElement, useRef } from "react";
import { DRAG_THRESHOLD_PX, dropTargetAt, usePointerDrag, type PointerDragSpec } from "./usePointerDrag";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

// jsdom implements neither pointer capture nor PointerEvent's button fields.
class FakePointerEvent extends MouseEvent {
  pointerId: number;
  constructor(type: string, init: MouseEventInit & { pointerId?: number } = {}) {
    super(type, { bubbles: true, cancelable: true, ...init });
    this.pointerId = init.pointerId ?? 1;
  }
}
(globalThis as unknown as { PointerEvent: typeof MouseEvent }).PointerEvent = FakePointerEvent;

// jsdom has no layout, so it ships no hit-testing either. The hook only ever
// hands the result to `dropTargetAt`, which is covered on its own below.
if (!document.elementFromPoint) {
  (document as unknown as { elementFromPoint: () => Element | null }).elementFromPoint = () => null;
}

let container: HTMLDivElement;
let root: Root;
let captured: number[];

beforeEach(() => {
  captured = [];
  HTMLElement.prototype.setPointerCapture = function (id: number) {
    captured.push(id);
  };
  HTMLElement.prototype.releasePointerCapture = function (id: number) {
    captured = captured.filter((c) => c !== id);
  };
  HTMLElement.prototype.hasPointerCapture = function (id: number) {
    return captured.includes(id);
  };
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
});

/** Mounts a single draggable box wired to `spec`, plus a click counter so the
 *  "a drag must not also click" rule is observable. */
function mountBox(spec: PointerDragSpec) {
  const clicks: string[] = [];
  function Box() {
    const { onPointerDown, didDrag } = usePointerDrag(spec);
    const n = useRef(0);
    return createElement("div", {
      id: "box",
      onPointerDown,
      onClick: () => {
        if (didDrag()) return;
        clicks.push("click" + ++n.current);
      },
    });
  }
  act(() => root.render(createElement(Box)));
  return { box: container.querySelector("#box") as HTMLElement, clicks };
}

function down(el: HTMLElement, x: number, y: number, button = 0) {
  act(() => {
    el.dispatchEvent(new FakePointerEvent("pointerdown", { clientX: x, clientY: y, button }));
  });
}
function move(x: number, y: number) {
  act(() => {
    window.dispatchEvent(new FakePointerEvent("pointermove", { clientX: x, clientY: y }));
  });
}
function up(x: number, y: number) {
  act(() => {
    window.dispatchEvent(new FakePointerEvent("pointerup", { clientX: x, clientY: y }));
  });
}
function click(el: HTMLElement) {
  act(() => {
    el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

describe("usePointerDrag", () => {
  it("keeps a press under the threshold a plain click — no drag callbacks", () => {
    const spec = { onStart: vi.fn(), onMove: vi.fn(), onDrop: vi.fn(), onCancel: vi.fn() };
    const { box, clicks } = mountBox(spec);
    down(box, 100, 100);
    move(100 + DRAG_THRESHOLD_PX - 1, 100);
    up(100 + DRAG_THRESHOLD_PX - 1, 100);
    click(box);
    expect(spec.onStart).not.toHaveBeenCalled();
    expect(spec.onDrop).not.toHaveBeenCalled();
    expect(spec.onCancel).not.toHaveBeenCalled();
    expect(clicks).toEqual(["click1"]);
  });

  it("starts once past the threshold, reports moves, and drops on release", () => {
    const spec = { onStart: vi.fn(), onMove: vi.fn(), onDrop: vi.fn(), onCancel: vi.fn() };
    const { box } = mountBox(spec);
    down(box, 100, 100);
    move(100 + DRAG_THRESHOLD_PX + 1, 100);
    move(200, 140);
    up(200, 140);
    expect(spec.onStart).toHaveBeenCalledTimes(1);
    expect(spec.onMove).toHaveBeenCalledTimes(2); // the starting move counts too
    expect(spec.onDrop).toHaveBeenCalledTimes(1);
    expect(spec.onDrop.mock.calls[0][0]).toMatchObject({ x: 200, y: 140 });
    expect(spec.onCancel).not.toHaveBeenCalled();
  });

  it("swallows the click that follows a completed drag", () => {
    const { box, clicks } = mountBox({});
    down(box, 100, 100);
    move(200, 100);
    up(200, 100);
    click(box); // the browser fires this after the drag's pointerup
    expect(clicks).toEqual([]);
    // …and only that one: the next real click still registers.
    down(box, 100, 100);
    up(100, 100);
    click(box);
    expect(clicks).toEqual(["click1"]);
  });

  it("cancels on Escape instead of dropping", () => {
    const spec = { onDrop: vi.fn(), onCancel: vi.fn() };
    const { box } = mountBox(spec);
    down(box, 100, 100);
    move(200, 100);
    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape", bubbles: true }));
    });
    up(200, 100);
    expect(spec.onCancel).toHaveBeenCalledTimes(1);
    expect(spec.onDrop).not.toHaveBeenCalled();
  });

  it("ignores non-primary buttons so right/middle click keep their meaning", () => {
    const spec = { onStart: vi.fn() };
    const { box } = mountBox(spec);
    down(box, 100, 100, 2);
    move(200, 100);
    up(200, 100);
    expect(spec.onStart).not.toHaveBeenCalled();
  });

  it("releases pointer capture when the drag ends", () => {
    const { box } = mountBox({});
    down(box, 100, 100);
    move(200, 100);
    expect(box.hasPointerCapture(1)).toBe(true);
    up(200, 100);
    expect(box.hasPointerCapture(1)).toBe(false);
  });

  it("stops the drag when onStart refuses it", () => {
    const spec = { onStart: () => false as const, onMove: vi.fn(), onDrop: vi.fn() };
    const { box } = mountBox(spec);
    down(box, 100, 100);
    move(200, 100);
    up(200, 100);
    expect(spec.onMove).not.toHaveBeenCalled();
    expect(spec.onDrop).not.toHaveBeenCalled();
  });
});

describe("dropTargetAt", () => {
  it("reads the nearest ancestor's marker attribute", () => {
    const zone = document.createElement("div");
    zone.setAttribute("data-group-id", "g2");
    const child = document.createElement("span");
    zone.appendChild(child);
    document.body.appendChild(zone);
    expect(dropTargetAt(child, "data-group-id")).toBe("g2");
    expect(dropTargetAt(zone, "data-group-id")).toBe("g2");
    expect(dropTargetAt(null, "data-group-id")).toBe(null);
    expect(dropTargetAt(document.body, "data-group-id")).toBe(null);
    zone.remove();
  });
});
