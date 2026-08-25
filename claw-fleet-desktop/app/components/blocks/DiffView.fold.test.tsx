// @vitest-environment jsdom
// A long edit must not take over the transcript. The inline diff folds after
// MAX_INLINE_ROWS and offers two ways out — unfold in place, or lift the whole
// thing into a full-screen sheet. Both paths are asserted here because the
// failure mode is silent: a fold that never unfolds still *looks* fine, it just
// hides the rest of the change forever.
import { afterEach, describe, expect, it } from "vitest";
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

import "../../i18n";
import { DiffView } from "./DiffView";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const MAX_INLINE_ROWS = 14;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  document.querySelectorAll("[role=dialog]").forEach((n) => n.remove());
});

/** A brand-new file of `lines` rows — every row is an addition, so the row
 *  count is exactly predictable and independent of the LCS diff. */
function mount(lines: number) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  const after = Array.from({ length: lines }, (_, i) => `line ${i + 1}`).join("\n");
  act(() => {
    root!.render(
      createElement(DiffView, { filePath: "src/app.ts", before: null, after }),
    );
  });
  return container;
}

const rowCount = (el: HTMLElement) =>
  el.querySelectorAll('[class*="row_add"], [class*="row_del"]').length;

const unfoldBtn = (el: HTMLElement) =>
  [...el.querySelectorAll("button")].find((b) => /\d/.test(b.textContent ?? "") &&
    !b.getAttribute("aria-label"));

describe("DiffView fold", () => {
  it("renders a short diff whole, with no unfold control", () => {
    const el = mount(6);

    expect(rowCount(el)).toBe(6);
    expect(unfoldBtn(el)).toBeUndefined();
  });

  it("folds a long diff to the cap and offers the rest", () => {
    const el = mount(40);

    expect(rowCount(el)).toBe(MAX_INLINE_ROWS);
    const btn = unfoldBtn(el);
    expect(btn).toBeDefined();
    // The control names how much is still hidden, not just "more".
    expect(btn!.textContent).toContain(String(40 - MAX_INLINE_ROWS));
  });

  it("shows every row once unfolded", () => {
    const el = mount(40);
    act(() => unfoldBtn(el)!.click());

    expect(rowCount(el)).toBe(40);
    expect(unfoldBtn(el)).toBeUndefined();
  });

  it("reports the change size in the path bar without unfolding", () => {
    const el = mount(40);

    // 40 added, 0 removed — readable while the body is still folded.
    expect(el.textContent).toContain("+40");
    expect(rowCount(el)).toBe(MAX_INLINE_ROWS);
  });

  it("lifts the full diff into a sheet, uncapped, and closes again", () => {
    const el = mount(40);
    const maximize = [...el.querySelectorAll("button")].find(
      (b) => b.textContent === "⤢",
    );
    expect(maximize).toBeDefined();

    act(() => maximize!.click());
    const sheet = document.querySelector("[role=dialog]") as HTMLElement | null;
    expect(sheet).not.toBeNull();
    // The sheet's copy is `full`: no cap, no nested maximize control.
    expect(rowCount(sheet!)).toBe(40);
    expect([...sheet!.querySelectorAll("button")].some((b) => b.textContent === "⤢")).toBe(false);

    const close = sheet!.querySelector("[aria-label]") as HTMLButtonElement;
    act(() => close.click());
    expect(document.querySelector("[role=dialog]")).toBeNull();
  });
});
