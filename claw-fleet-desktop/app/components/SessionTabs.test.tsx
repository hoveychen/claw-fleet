// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

vi.mock("../store", () => ({
  useSessionsStore: (selector: (state: { sessions: never[] }) => unknown) =>
    selector({ sessions: [] }),
}));

import "../i18n";
import { SessionTabs, type TabItem } from "./SessionTabs";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
// jsdom ships no ResizeObserver, and SessionTabs constructs one in a mount
// effect to track strip overflow — without this stub every test in the file dies
// with `ReferenceError: ResizeObserver is not defined` before reaching its
// assertions. Same stub as ResumeComposer.test.tsx.
(globalThis as unknown as { ResizeObserver: typeof ResizeObserver }).ResizeObserver = class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const tabs: TabItem[] = [
  { id: "first", session: null, label: "First" },
  { id: "second", session: null, label: "Second" },
  { id: "third", session: null, label: "Third" },
];

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeAll(() => {
  HTMLElement.prototype.scrollIntoView = vi.fn();
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function renderTabs(onActivate: (id: string) => void) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() =>
    root!.render(
      <SessionTabs
        tabs={tabs}
        activeId="first"
        groupId="g1"
        isActiveGroup
        splittable
        drag={null}
        onDragStart={vi.fn()}
        onDragEnd={vi.fn()}
        onDragHover={vi.fn()}
        onDropTab={vi.fn()}
        onActivate={onActivate}
        onClose={vi.fn()}
        onCloseOthers={vi.fn()}
        onCloseRight={vi.fn()}
        onCloseAll={vi.fn()}
        onReorder={vi.fn()}
        onSplitRight={vi.fn()}
        onSplitDown={vi.fn()}
      />,
    ),
  );
}

describe("SessionTabs keyboard navigation", () => {
  it("uses roving tab stops and activates the next tab with ArrowRight", () => {
    const onActivate = vi.fn();
    renderTabs(onActivate);
    const rendered = [...container!.querySelectorAll<HTMLElement>('[role="tab"]')];

    expect(rendered.map((tab) => tab.tabIndex)).toEqual([0, -1, -1]);
    act(() =>
      rendered[0].dispatchEvent(new KeyboardEvent("keydown", { key: "ArrowRight", bubbles: true })),
    );

    expect(onActivate).toHaveBeenLastCalledWith("second");
    expect(document.activeElement).toBe(rendered[1]);
  });

  it("activates tabs with Space and supports Home/End navigation", () => {
    const onActivate = vi.fn();
    renderTabs(onActivate);
    const rendered = [...container!.querySelectorAll<HTMLElement>('[role="tab"]')];

    act(() =>
      rendered[0].dispatchEvent(new KeyboardEvent("keydown", { key: " ", bubbles: true })),
    );
    expect(onActivate).toHaveBeenLastCalledWith("first");

    act(() =>
      rendered[0].dispatchEvent(new KeyboardEvent("keydown", { key: "End", bubbles: true })),
    );
    expect(onActivate).toHaveBeenLastCalledWith("third");
    expect(document.activeElement).toBe(rendered[2]);

    act(() =>
      rendered[2].dispatchEvent(new KeyboardEvent("keydown", { key: "Home", bubbles: true })),
    );
    expect(onActivate).toHaveBeenLastCalledWith("first");
    expect(document.activeElement).toBe(rendered[0]);
  });
});

// ── Pointer-driven tab drag ──────────────────────────────────────────────────
// The strip drags with pointer events, not the HTML5 drag API — that API is
// inert inside the app's Tauri webview (see usePointerDrag.ts). These lock in
// the wiring that replaced it: the `data-group-id` / `data-tab-id` markers the
// drag hit-tests against, and what each landing spot means.

class FakePointerEvent extends MouseEvent {
  pointerId: number;
  constructor(type: string, init: MouseEventInit & { pointerId?: number } = {}) {
    super(type, { bubbles: true, cancelable: true, ...init });
    this.pointerId = init.pointerId ?? 1;
  }
}
(globalThis as unknown as { PointerEvent: typeof MouseEvent }).PointerEvent = FakePointerEvent;

// jsdom has no layout, so the drag's hit-test is driven by hand: `hit` is
// whatever the pointer is currently "over".
let hit: Element | null = null;
(document as unknown as { elementFromPoint: () => Element | null }).elementFromPoint = () => hit;

function stubPointerCapture() {
  let held: number[] = [];
  HTMLElement.prototype.setPointerCapture = function (id: number) { held.push(id); };
  HTMLElement.prototype.releasePointerCapture = function (id: number) {
    held = held.filter((h) => h !== id);
  };
  HTMLElement.prototype.hasPointerCapture = function (id: number) { return held.includes(id); };
}

/** Two strips side by side, wired the way HistoryView wires them: one drag
 *  state shared by both, so a drag starting in g1 can land in g2. */
function renderTwoGroups() {
  stubPointerCapture();
  const calls = {
    onDropTab: vi.fn(),
    onReorder: vi.fn(),
    onDragHover: vi.fn(),
    onDragEnd: vi.fn(),
    onActivate: vi.fn(),
  };
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const common = {
    isActiveGroup: true,
    splittable: true,
    drag: null,
    onDragStart: vi.fn(),
    onDragEnd: calls.onDragEnd,
    onDragHover: calls.onDragHover,
    onDropTab: calls.onDropTab,
    onActivate: calls.onActivate,
    onClose: vi.fn(),
    onCloseOthers: vi.fn(),
    onCloseRight: vi.fn(),
    onCloseAll: vi.fn(),
    onReorder: calls.onReorder,
    onSplitRight: vi.fn(),
    onSplitDown: vi.fn(),
  };
  act(() =>
    root!.render(
      <>
        <SessionTabs {...common} groupId="g1" tabs={tabs} activeId="first" />
        <SessionTabs {...common} groupId="g2" tabs={[]} activeId={null} />
      </>,
    ),
  );
  const strips = [...container.querySelectorAll<HTMLElement>("[data-group-id]")];
  return { calls, g1Strip: strips[0], g2Strip: strips[1] };
}

function tabEl(id: string): HTMLElement {
  return container!.querySelector<HTMLElement>(`[data-tab-id="${id}"]`)!;
}

/** Press on `from`, travel to wherever `hit` points, release. */
function dragTo(from: HTMLElement, target: Element | null, release = true) {
  hit = target;
  act(() => {
    from.dispatchEvent(new FakePointerEvent("pointerdown", { clientX: 10, clientY: 10, button: 0 }));
  });
  act(() => {
    window.dispatchEvent(new FakePointerEvent("pointermove", { clientX: 90, clientY: 10 }));
  });
  if (!release) return;
  act(() => {
    window.dispatchEvent(new FakePointerEvent("pointerup", { clientX: 90, clientY: 10 }));
  });
}

describe("SessionTabs pointer drag", () => {
  it("moves a tab to another group when released over that group's strip", () => {
    const { calls, g2Strip } = renderTwoGroups();
    dragTo(tabEl("second"), g2Strip);
    expect(calls.onDropTab).toHaveBeenCalledWith("g2", null);
    expect(calls.onReorder).not.toHaveBeenCalled();
  });

  it("drops before the tab under the cursor when the target group has tabs", () => {
    const { calls } = renderTwoGroups();
    // Pretend g2 holds "third": releasing over it must insert *before* it.
    const thirdInOtherGroup = document.createElement("div");
    thirdInOtherGroup.setAttribute("data-tab-id", "third");
    const otherStrip = document.createElement("div");
    otherStrip.setAttribute("data-group-id", "g2");
    otherStrip.appendChild(thirdInOtherGroup);
    document.body.appendChild(otherStrip);
    dragTo(tabEl("first"), thirdInOtherGroup);
    expect(calls.onDropTab).toHaveBeenCalledWith("g2", "third");
    otherStrip.remove();
  });

  it("reorders live inside its own group instead of dropping", () => {
    const { calls } = renderTwoGroups();
    dragTo(tabEl("first"), tabEl("third"));
    expect(calls.onReorder).toHaveBeenCalledWith("first", "third");
    expect(calls.onDropTab).not.toHaveBeenCalled();
  });

  it("reports the hovered group so the destination strip can light up", () => {
    const { calls, g2Strip } = renderTwoGroups();
    dragTo(tabEl("second"), g2Strip, false);
    expect(calls.onDragHover).toHaveBeenCalledWith("g2");
  });

  it("keeps a plain click activating the tab", () => {
    const { calls } = renderTwoGroups();
    const el = tabEl("second");
    act(() => {
      el.dispatchEvent(new FakePointerEvent("pointerdown", { clientX: 10, clientY: 10, button: 0 }));
      window.dispatchEvent(new FakePointerEvent("pointerup", { clientX: 10, clientY: 10 }));
      el.dispatchEvent(new MouseEvent("click", { bubbles: true }));
    });
    expect(calls.onActivate).toHaveBeenCalledWith("second");
    expect(calls.onDropTab).not.toHaveBeenCalled();
  });

  it("does not activate the tab on the click that follows a drag", () => {
    const { calls, g2Strip } = renderTwoGroups();
    const el = tabEl("second");
    dragTo(el, g2Strip);
    act(() => el.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(calls.onActivate).not.toHaveBeenCalled();
  });
});
