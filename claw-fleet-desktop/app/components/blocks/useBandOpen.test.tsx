// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { useBandOpen } from "./useBandOpen";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

/** Probe that renders the hook's state and exposes its setter to a click. */
function Probe({ defaultOpen, forceOpen }: { defaultOpen: boolean; forceOpen: boolean }) {
  const [open, setOpen] = useBandOpen(defaultOpen, forceOpen);
  return (
    <button data-testid="band" onClick={() => setOpen((o) => !o)}>
      {open ? "open" : "closed"}
    </button>
  );
}

function render(defaultOpen: boolean, forceOpen = false) {
  act(() => {
    root!.render(<Probe defaultOpen={defaultOpen} forceOpen={forceOpen} />);
  });
}

function state(): string {
  return container!.querySelector('[data-testid="band"]')!.textContent!;
}

function click() {
  act(() => {
    container!
      .querySelector('[data-testid="band"]')!
      .dispatchEvent(new MouseEvent("click", { bubbles: true }));
  });
}

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root!.unmount());
  container!.remove();
  container = null;
  root = null;
});

describe("work-run band open state", () => {
  it("opens when the band becomes the live tail", () => {
    render(false);
    expect(state()).toBe("closed");
    render(true);
    expect(state()).toBe("open");
  });

  it("stays open when the live signal drops — a turn's momentary flips must not collapse it", () => {
    render(true);
    expect(state()).toBe("open");
    // The agent writes one prose record: the band is no longer the last render
    // unit, so `defaultOpen` goes false mid-turn.
    render(false);
    expect(state()).toBe("open");
    // …and back, as the next tool records form a run again.
    render(true);
    expect(state()).toBe("open");
  });

  it("keeps a manual collapse across later signal flips", () => {
    render(true);
    click();
    expect(state()).toBe("closed");
    render(false);
    expect(state()).toBe("closed");
  });

  it("keeps a manual expand when the live signal is already false", () => {
    render(false);
    click();
    expect(state()).toBe("open");
    render(false);
    expect(state()).toBe("open");
  });

  it("opens on an active search hit and leaves it open after the reader steps off", () => {
    render(false, false);
    render(false, true);
    expect(state()).toBe("open");
    render(false, false);
    expect(state()).toBe("open");
  });
});
