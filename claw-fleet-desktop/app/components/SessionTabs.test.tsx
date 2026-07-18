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
        onActivate={onActivate}
        onClose={vi.fn()}
        onCloseOthers={vi.fn()}
        onCloseRight={vi.fn()}
        onCloseAll={vi.fn()}
        onReorder={vi.fn()}
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
