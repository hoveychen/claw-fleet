// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

import "../i18n";
import { AuxDocBar, AuxPane } from "./AuxDocBar";
import { EXIT_MS } from "./Presence";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  vi.useRealTimers();
});

/** Stands in for a reader whose state (a picked version, a loaded read) must
 *  survive the move into the overlay and back. */
function Counter() {
  const [n, setN] = useState(0);
  return (
    <button type="button" data-testid="counter" onClick={() => setN(n + 1)}>
      {n}
    </button>
  );
}

function mount() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() =>
    root!.render(
      <AuxPane menuItems={[]} className="pane">
        <AuxDocBar kind="artifact" icon={null} title="poster" facts={[]} />
        <Counter />
      </AuxPane>,
    ),
  );
}

const dialog = () => document.body.querySelector('[role="dialog"]');
const counter = () => document.body.querySelector<HTMLButtonElement>('[data-testid="counter"]')!;
const maxButton = () =>
  document.body.querySelector<HTMLButtonElement>('button[aria-pressed]')!;

describe("AuxPane maximize", () => {
  it("moves the whole card into the overlay without remounting it, and Esc restores", () => {
    vi.useFakeTimers();
    mount();
    act(() => counter().click());
    expect(counter().textContent).toBe("1");
    expect(dialog()).toBeNull();

    act(() => maxButton().click());
    expect(dialog()).not.toBeNull();
    expect(dialog()!.contains(counter())).toBe(true);
    expect(counter().textContent).toBe("1");
    expect(maxButton().getAttribute("aria-pressed")).toBe("true");

    act(() => {
      window.dispatchEvent(new KeyboardEvent("keydown", { key: "Escape" }));
    });
    act(() => vi.advanceTimersByTime(EXIT_MS + 10));
    expect(dialog()).toBeNull();
    expect(container!.contains(counter())).toBe(true);
    expect(counter().textContent).toBe("1");
  });

  it("restores from the button too", () => {
    vi.useFakeTimers();
    mount();
    act(() => maxButton().click());
    act(() => maxButton().click());
    act(() => vi.advanceTimersByTime(EXIT_MS + 10));
    expect(dialog()).toBeNull();
    expect(container!.contains(counter())).toBe(true);
  });
});
