// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@tauri-apps/plugin-clipboard-manager", () => ({ writeText: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => ({ invoke: vi.fn(() => new Promise(() => {})) }));

import { TextBlock } from "./TextBlock";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  // Present but inert: the component must defer chunks rather than take the
  // "no IntersectionObserver, show everything" escape route.
  class FakeIO {
    observe() {}
    disconnect() {}
  }
  (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver = FakeIO;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  delete (globalThis as unknown as { IntersectionObserver?: unknown }).IntersectionObserver;
});

/** Repeat `unit` until at least `bytes` long. */
function bulk(unit: string, bytes: number): string {
  let out = "";
  while (out.length < bytes) out += unit;
  return out;
}

/** Well past the 32 KiB chunk target, with a recognisable marker near the end.
 *  The marker is the *second to last* paragraph on purpose: while streaming,
 *  TextBlock drops the last one to avoid flicker mid-token, so a marker in the
 *  final paragraph would be missing for reasons that have nothing to do with
 *  chunking. */
const longText =
  bulk("A paragraph of prose that goes on for a while.\n\n", 120_000) +
  "THE-VERY-END\n\nstill arriving\n";

describe("TextBlock progressive rendering", () => {
  it("defers the tail of a long history message", () => {
    act(() => root!.render(<TextBlock text={longText} />));
    const text = container!.textContent ?? "";
    expect(text).toContain("A paragraph of prose");
    expect(text).not.toContain("THE-VERY-END");
  });

  it("never chunks a message that is still streaming", () => {
    // The reader is watching it grow; folding it to the first chunks would
    // hide text that was on screen a moment ago.
    act(() => root!.render(<TextBlock text={longText} isPartial />));
    expect(container!.textContent).toContain("THE-VERY-END");
  });

  it("keeps a streamed message whole after the stream ends", () => {
    // The regression this guards: `isPartial` flips to false when the turn
    // finishes, and a naive implementation would collapse the message the
    // reader is in the middle of back to its first two chunks.
    act(() => root!.render(<TextBlock text={longText} isPartial />));
    expect(container!.textContent).toContain("THE-VERY-END");
    act(() => root!.render(<TextBlock text={longText} />));
    expect(container!.textContent).toContain("THE-VERY-END");
  });

  it("still highlights search terms", () => {
    // Highlighting is injected through the `components` map, which now travels
    // to every chunk — a wiring mistake there loses it silently.
    act(() =>
      root!.render(<TextBlock text="hello world" searchTerms={["world"]} />),
    );
    const marks = container!.querySelectorAll("mark");
    expect(marks.length).toBe(1);
    expect(marks[0].textContent).toBe("world");
  });

  it("highlights search terms in a deferred document's visible chunks", () => {
    act(() =>
      root!.render(<TextBlock text={longText} searchTerms={["paragraph"]} />),
    );
    expect(container!.querySelectorAll("mark").length).toBeGreaterThan(0);
  });
});
