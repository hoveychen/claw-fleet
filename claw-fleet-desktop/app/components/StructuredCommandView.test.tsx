// @vitest-environment jsdom
import { afterEach, describe, expect, it } from "vitest";
import { createElement } from "react";
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import type { CommandView } from "../types";

import "../i18n";
import { StructuredCommandView } from "./StructuredCommandView";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;
afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function render(view: CommandView) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(createElement(StructuredCommandView, { command: "raw", view })));
  return container;
}

describe("StructuredCommandView", () => {
  it("shows redirect targets and a data heredoc body", () => {
    const el = render({
      leaves: [
        { argv: ["cat"], redirects: ["> notes.md", "<<"], heredoc: "hello body\n" },
        { argv: ["ls"], redirects: ["2>&1"] },
      ],
      connectors: ["semi"],
    });
    const text = el.textContent ?? "";
    expect(text).toContain("> notes.md");
    expect(text).toContain("2>&1");
    expect(text).toContain("hello body");
    const details = el.querySelector("details");
    expect(details?.open).toBe(true);
  });

  it("collapses a long heredoc body", () => {
    const body = Array.from({ length: 40 }, (_, i) => `line ${i}`).join("\n") + "\n";
    const el = render({
      leaves: [{ argv: ["cat"], redirects: ["> big.md", "<<"], heredoc: body }],
      connectors: [],
    });
    const details = el.querySelector("details");
    expect(details?.open).toBe(false);
    expect(details?.querySelector("summary")?.textContent).toMatch(/40/);
  });
});
