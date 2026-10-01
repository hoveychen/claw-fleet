// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useDelayedFlag } from "./useDelayedFlag";
import { usePending } from "./usePending";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let host: HTMLDivElement | null = null;
let seen: boolean[] = [];

function Probe({ active }: { active: boolean }) {
  seen.push(useDelayedFlag(active, 200, 300));
  return null;
}

const last = () => seen[seen.length - 1];
const render = (active: boolean) => act(() => root!.render(<Probe active={active} />));
const advance = (ms: number) => act(() => vi.advanceTimersByTime(ms));

beforeEach(() => {
  vi.useFakeTimers();
  seen = [];
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  vi.useRealTimers();
});

describe("useDelayedFlag", () => {
  it("never shows a loader for a wait shorter than the delay", () => {
    render(true);
    advance(150);
    render(false);
    advance(1000);
    expect(seen.every((v) => v === false)).toBe(true);
  });

  it("shows after the delay and holds for the minimum time", () => {
    render(true);
    advance(200);
    expect(last()).toBe(true);
    render(false);
    advance(100);
    expect(last()).toBe(true);
    advance(200);
    expect(last()).toBe(false);
  });

  it("hides immediately when it has already been visible long enough", () => {
    render(true);
    advance(800);
    expect(last()).toBe(true);
    render(false);
    expect(last()).toBe(false);
  });
});

describe("usePending", () => {
  it("drops re-entrant calls and clears pending when done", async () => {
    vi.useRealTimers();
    let resolve!: () => void;
    const fn = vi.fn(() => new Promise<void>((r) => (resolve = r)));
    let state: [boolean, () => Promise<void | undefined>] | null = null;
    function P() {
      state = usePending(fn);
      return null;
    }
    act(() => root!.render(<P />));
    let first!: Promise<unknown>;
    act(() => {
      first = state![1]();
      void state![1]();
    });
    expect(fn).toHaveBeenCalledTimes(1);
    expect(state![0]).toBe(true);
    await act(async () => {
      resolve();
      await first;
    });
    expect(state![0]).toBe(false);
  });
});
