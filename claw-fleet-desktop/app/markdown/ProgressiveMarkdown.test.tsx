// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

/** Count how often react-markdown is asked to render. It re-parses on every
 *  render, so this count is the parse count — the thing the whole feature is
 *  about. */
const parses = vi.fn();
/** Props of the most recent react-markdown render, so a test can assert what
 *  the component actually handed down rather than that it rendered at all. */
let lastProps: Record<string, unknown> = {};
vi.mock("react-markdown", () => ({
  default: (props: { children?: string } & Record<string, unknown>) => {
    parses();
    lastProps = props;
    return <div data-chunk>{props.children}</div>;
  },
}));

/** Spy over the real splitter: a small body must not even reach it. */
const chunkSpy = vi.fn();
vi.mock("./chunkMarkdown", async (importOriginal) => {
  const actual = await importOriginal<typeof import("./chunkMarkdown")>();
  return {
    ...actual,
    chunkMarkdown: (src: string, target?: number) => {
      chunkSpy(src);
      return actual.chunkMarkdown(src, target);
    },
  };
});

import "../i18n";
import { ProgressiveMarkdown } from "./ProgressiveMarkdown";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;
/** Callbacks of every IntersectionObserver the component created. */
let observers: Array<(entries: Array<{ isIntersecting: boolean }>) => void> = [];

beforeEach(() => {
  observers = [];
  class FakeIO {
    constructor(cb: (entries: Array<{ isIntersecting: boolean }>) => void) {
      observers.push(cb);
    }
    observe() {}
    disconnect() {}
  }
  (globalThis as unknown as { IntersectionObserver: unknown }).IntersectionObserver = FakeIO;
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  parses.mockReset();
  chunkSpy.mockReset();
  lastProps = {};
  delete (globalThis as unknown as { IntersectionObserver?: unknown }).IntersectionObserver;
});

function render(body: string) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<ProgressiveMarkdown body={body} components={{}} />));
  return container!;
}

/** Repeat `unit` until at least `bytes` long. */
function bulk(unit: string, bytes: number): string {
  let out = "";
  while (out.length < bytes) out += unit;
  return out;
}

/** ~10 chunks at the default 32 KiB target. */
const longDoc =
  "# Long\n\n" +
  Array.from(
    { length: 320 },
    (_, i) => `## Section ${i}\n\n${bulk(`para ${i} text.\n\n`, 1000)}`,
  ).join("");

function scrollToBottom() {
  act(() => {
    for (const cb of observers) cb([{ isIntersecting: true }]);
  });
}

describe("ProgressiveMarkdown", () => {
  it("parses only the first chunks on open, not the whole document", () => {
    const el = render(longDoc);
    // The point of the feature: opening costs a couple of chunks, not ~10.
    expect(parses.mock.calls.length).toBeLessThanOrEqual(2);
    expect(el.querySelectorAll("[data-chunk]").length).toBeLessThanOrEqual(2);
    // …and something is actually on screen.
    expect(el.textContent).toContain("Long");
  });

  it("appends the next chunk when the foot of the document comes into view", () => {
    const el = render(longDoc);
    const before = el.querySelectorAll("[data-chunk]").length;
    scrollToBottom();
    expect(el.querySelectorAll("[data-chunk]").length).toBe(before + 1);
  });

  it("does not re-parse the chunks already on screen when appending", () => {
    render(longDoc);
    const afterOpen = parses.mock.calls.length;
    scrollToBottom();
    // One more parse for the one new chunk — the earlier ones are memoised.
    expect(parses.mock.calls.length).toBe(afterOpen + 1);
  });

  it("reaches the whole document by scrolling", () => {
    const el = render(longDoc);
    // Keep pulling until the sentinel is gone; it disappears only when every
    // chunk is rendered, so a document that could strand a reader loops here.
    for (let i = 0; i < 100 && observers.length > 0; i += 1) {
      const had = el.querySelectorAll("[data-chunk]").length;
      scrollToBottom();
      if (el.querySelectorAll("[data-chunk]").length === had) break;
    }
    expect(el.textContent).toContain("Section 319");
  });

  it("offers a one-click escape hatch for the rest of the document", () => {
    // Scrolling can't serve in-page find or select-all — this can.
    const el = render(longDoc);
    const button = el.querySelector("button");
    expect(button).not.toBeNull();
    act(() => button!.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    expect(el.textContent).toContain("Section 319");
    expect(el.querySelector("button")).toBeNull();
  });

  it("renders a short document whole, with no sentinel", () => {
    const el = render("# Small\n\nOne paragraph.\n");
    expect(parses.mock.calls.length).toBe(1);
    expect(el.querySelector("button")).toBeNull();
    expect(el.textContent).toContain("Small");
  });

  it("hands a small body straight through, unsplit", () => {
    // Chat is thousands of small messages; scanning each for cut points would
    // be pure overhead. Anything that cannot need a second chunk skips it.
    const src = "para one\n\npara two\n\n" + bulk("- bullet\n", 2_000);
    const el = render(src);
    expect(chunkSpy).not.toHaveBeenCalled();
    expect(parses.mock.calls.length).toBe(1);
    expect(el.querySelectorAll("[data-chunk]")[0]?.textContent).toBe(src);
  });

  it("does split a body large enough to need it", () => {
    render(longDoc);
    expect(chunkSpy).toHaveBeenCalledTimes(1);
  });

  it("passes the caller's remark plugins down to every chunk", () => {
    // TextBlock adds the wiki-link plugin only when it has a wiki context; if
    // this component kept a hard-coded chain it would silently drop it, and
    // `[[slug]]` refs would render as literal text with nothing to notice.
    const marker = () => () => {};
    render("# hi\n");
    expect(lastProps.remarkPlugins).toBeDefined();
    expect(lastProps.remarkPlugins).not.toContain(marker);

    act(() =>
      root!.render(
        <ProgressiveMarkdown body="# hi\n" components={{}} remarkPlugins={[marker]} />,
      ),
    );
    expect(lastProps.remarkPlugins).toContain(marker);
  });

  it("does not cut an inline SVG apart", () => {
    // Blank lines inside a drawing are why `normalizeSvgBlankLines` exists —
    // and they look exactly like the blank lines the splitter cuts at. A cut
    // between `<svg>` and `</svg>` leaves two broken halves.
    // The drawing itself is bigger than one chunk, so it must straddle a cut.
    const svg =
      '<svg viewBox="0 0 10 10">\n\n' +
      bulk('  <rect width="10" height="10"/>\n\n', 50_000) +
      "</svg>";
    const src = bulk("para text here\n\n", 20_000) + svg + "\n\ntail para\n";
    const el = render(src);
    const rest = el.querySelector("button");
    if (rest) act(() => rest.dispatchEvent(new MouseEvent("click", { bubbles: true })));
    const bodies = [...el.querySelectorAll("[data-chunk]")].map((n) => n.textContent ?? "");
    expect(bodies.length).toBeGreaterThan(1); // the split really did happen
    const opener = bodies.filter((b) => b.includes("<svg"));
    expect(opener).toHaveLength(1);
    expect(opener[0]).toContain("</svg>");
  });

  it("shows everything when the platform has no IntersectionObserver", () => {
    // Without the fallback the reader would be stuck at the first two chunks
    // with no way to scroll further.
    delete (globalThis as unknown as { IntersectionObserver?: unknown }).IntersectionObserver;
    const el = render(longDoc);
    expect(el.textContent).toContain("Section 319");
  });
});
