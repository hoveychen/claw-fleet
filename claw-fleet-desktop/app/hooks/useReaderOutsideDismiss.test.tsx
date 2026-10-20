// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { isReaderOutsideTarget, useReaderOutsideDismiss } from "./useReaderOutsideDismiss";

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let appRoot: HTMLDivElement;
let prose: HTMLParagraphElement;
let readerHost: HTMLDivElement;
let readerText: HTMLSpanElement;
let rail: HTMLElement;
let overlay: HTMLDivElement;
let reactHost: HTMLDivElement;
let root: Root;

beforeEach(() => {
  appRoot = document.createElement("div");
  appRoot.id = "root";
  prose = document.createElement("p");
  readerHost = document.createElement("div");
  readerText = document.createElement("span");
  readerHost.appendChild(readerText);
  rail = document.createElement("aside");
  rail.setAttribute("data-reader-keep", "");
  rail.appendChild(document.createElement("button"));
  appRoot.append(prose, readerHost, rail);
  // A context menu / lightbox: portaled to <body>, outside #root.
  overlay = document.createElement("div");
  reactHost = document.createElement("div");
  document.body.append(appRoot, overlay, reactHost);
  root = createRoot(reactHost);
});

afterEach(() => {
  act(() => root.unmount());
  document.body.innerHTML = "";
});

describe("isReaderOutsideTarget", () => {
  it("counts the conversation and the nav as outside", () => {
    expect(isReaderOutsideTarget(prose, readerHost, appRoot)).toBe(true);
  });

  it("ignores the reader itself, keep zones and body-level overlays", () => {
    expect(isReaderOutsideTarget(readerText, readerHost, appRoot)).toBe(false);
    expect(isReaderOutsideTarget(rail.firstChild, readerHost, appRoot)).toBe(false);
    expect(isReaderOutsideTarget(overlay, readerHost, appRoot)).toBe(false);
  });
});

function Probe({ active, onDismiss }: { active: boolean; onDismiss: () => void }) {
  useReaderOutsideDismiss(active, readerHost, onDismiss);
  return null;
}

function press(target: Element, from: [number, number], to: [number, number] = from) {
  const opts = (x: number, y: number) => ({ bubbles: true, button: 0, clientX: x, clientY: y });
  target.dispatchEvent(new MouseEvent("pointerdown", opts(...from)));
  target.dispatchEvent(new MouseEvent("pointerup", opts(...to)));
}

describe("useReaderOutsideDismiss", () => {
  it("dismisses on a click outside, not inside", () => {
    const onDismiss = vi.fn();
    act(() => root.render(<Probe active onDismiss={onDismiss} />));
    press(readerText, [10, 10]);
    expect(onDismiss).not.toHaveBeenCalled();
    press(prose, [10, 10]);
    expect(onDismiss).toHaveBeenCalledTimes(1);
  });

  it("leaves a drag-select alone", () => {
    const onDismiss = vi.fn();
    act(() => root.render(<Probe active onDismiss={onDismiss} />));
    press(prose, [10, 10], [80, 12]);
    expect(onDismiss).not.toHaveBeenCalled();
  });

  it("does nothing while no reader is open", () => {
    const onDismiss = vi.fn();
    act(() => root.render(<Probe active={false} onDismiss={onDismiss} />));
    press(prose, [10, 10]);
    expect(onDismiss).not.toHaveBeenCalled();
  });
});
