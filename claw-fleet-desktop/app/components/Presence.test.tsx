// @vitest-environment jsdom
import { act } from "react";
import { createPortal } from "react-dom";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EXIT_MS, Presence, useExiting } from "./Presence";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let root: Root | null = null;
let host: HTMLDivElement | null = null;
let reduced = false;

function Dialog({ label }: { label: string }) {
  return <div className="dialog">{label}</div>;
}

function PortalMenu() {
  const exiting = useExiting();
  return createPortal(<div className="menu" data-exiting={exiting || undefined} />, document.body);
}

function render(node: React.ReactNode) {
  act(() => root!.render(node));
}

beforeEach(() => {
  vi.useFakeTimers();
  reduced = false;
  window.matchMedia = ((q: string) => ({ matches: reduced && q.includes("reduce") })) as unknown as typeof window.matchMedia;
  host = document.createElement("div");
  document.body.appendChild(host);
  root = createRoot(host);
});

afterEach(() => {
  act(() => root?.unmount());
  host?.remove();
  document.body.innerHTML = "";
  root = null;
  host = null;
  vi.useRealTimers();
});

describe("Presence", () => {
  it("renders nothing while closed", () => {
    render(<Presence when={false}><Dialog label="a" /></Presence>);
    expect(host!.innerHTML).toBe("");
  });

  it("shows children without the exiting marker while open", () => {
    render(<Presence when><Dialog label="a" /></Presence>);
    expect(host!.querySelector(".dialog")?.textContent).toBe("a");
    expect(host!.querySelector("[data-exiting]")).toBeNull();
  });

  it("keeps the last children mounted under data-exiting until the exit ends", () => {
    const target: string | null = "a";
    render(<Presence when={!!target}>{target && <Dialog label={target} />}</Presence>);
    // The call site's own condition is already false, so children are null.
    render(<Presence when={false}>{null}</Presence>);
    expect(host!.querySelector("[data-exiting] .dialog")?.textContent).toBe("a");
    act(() => vi.advanceTimersByTime(EXIT_MS + 1));
    expect(host!.innerHTML).toBe("");
  });

  it("does not remount the open children when the exit starts", () => {
    render(<Presence when><Dialog label="a" /></Presence>);
    const before = host!.querySelector(".dialog");
    render(<Presence when={false}><Dialog label="a" /></Presence>);
    expect(host!.querySelector(".dialog")).toBe(before);
  });

  it("shows the new children at once when reopened mid-exit", () => {
    render(<Presence when><Dialog label="a" /></Presence>);
    render(<Presence when={false}>{null}</Presence>);
    render(<Presence when><Dialog label="b" /></Presence>);
    expect(host!.querySelector("[data-exiting]")).toBeNull();
    expect(host!.querySelector(".dialog")?.textContent).toBe("b");
    act(() => vi.advanceTimersByTime(EXIT_MS + 1));
    expect(host!.querySelector(".dialog")?.textContent).toBe("b");
  });

  it("unmounts at once under prefers-reduced-motion", () => {
    reduced = true;
    render(<Presence when><Dialog label="a" /></Presence>);
    render(<Presence when={false}>{null}</Presence>);
    expect(host!.innerHTML).toBe("");
  });

  it("tells a portalled child it is exiting", () => {
    render(<Presence when><PortalMenu /></Presence>);
    expect(document.body.querySelector(".menu")?.hasAttribute("data-exiting")).toBe(false);
    render(<Presence when={false}><PortalMenu /></Presence>);
    expect(document.body.querySelector(".menu")?.hasAttribute("data-exiting")).toBe(true);
  });
});
