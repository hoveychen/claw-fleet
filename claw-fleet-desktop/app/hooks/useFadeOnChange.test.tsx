// @vitest-environment jsdom
import { act, useRef } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { useFadeOnChange } from "./useFadeOnChange";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const animate = vi.fn(() => ({ cancel: vi.fn() }));
let root: Root | null = null;
let host: HTMLDivElement | null = null;
let reduced = false;

function Probe({ k }: { k: string | null }) {
  const ref = useRef<HTMLDivElement>(null);
  useFadeOnChange(ref, k);
  return <div ref={ref} />;
}

function render(k: string | null) {
  act(() => root!.render(<Probe k={k} />));
}

beforeEach(() => {
  animate.mockClear();
  reduced = false;
  (HTMLElement.prototype as unknown as { animate: unknown }).animate = animate;
  window.matchMedia = ((q: string) => ({ matches: reduced && q.includes("reduce") })) as unknown as typeof window.matchMedia;
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  root = null;
  host = null;
});

describe("useFadeOnChange", () => {
  it("does not animate on the first render", () => {
    render("a");
    expect(animate).not.toHaveBeenCalled();
  });

  it("fades in when the key changes", () => {
    render("a");
    render("b");
    expect(animate).toHaveBeenCalledTimes(1);
  });

  it("does not animate on a re-render with the same key", () => {
    render("a");
    render("a");
    expect(animate).not.toHaveBeenCalled();
  });

  it("does not animate between nothing selected and a selection", () => {
    render(null);
    render("a");
    render(null);
    expect(animate).not.toHaveBeenCalled();
  });

  it("does not animate under prefers-reduced-motion", () => {
    reduced = true;
    render("a");
    render("b");
    expect(animate).not.toHaveBeenCalled();
  });
});
