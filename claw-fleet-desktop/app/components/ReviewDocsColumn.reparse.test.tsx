// @vitest-environment jsdom
import { act, useState } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("./AutoHeightFrame", () => ({ AutoHeightFrame: () => null }));

/** Stand-in for react-markdown that counts how often it is asked to render.
 *  The real one re-runs the whole remark/rehype chain on every call — there is
 *  no memo inside it — so this count IS the parse count. */
const renders = vi.fn();
vi.mock("react-markdown", () => ({
  default: ({ children }: { children?: string }) => {
    renders();
    return <div data-md>{children}</div>;
  },
}));

import "../i18n";
import { ReviewDocsColumn } from "./ReviewDocsColumn";
import type { ReviewDoc, ReviewDocContent } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
  invoke.mockReset();
  renders.mockReset();
});

// Module-level so the array identity is stable across parent re-renders, the
// way `active.request.reviewDocs` is stable across a card's lifetime.
const docs: ReviewDoc[] = [{ kind: "file", ref: "/Users/x/proj/TASKS.md", title: null }];

const content: ReviewDocContent = {
  format: "markdown",
  body: "# plan\n\n- [ ] P1\n",
  title: "TASKS.md",
  truncated: null,
};

describe("ReviewDocsColumn re-parse", () => {
  it("does not re-render the body when the parent re-renders for unrelated reasons", async () => {
    invoke.mockResolvedValue(content);
    let bump: (() => void) | null = null;

    function Parent() {
      // Stands in for DecisionPanel, which subscribes to the sessions store and
      // therefore re-renders on every 2s backend rescan.
      const [n, setN] = useState(0);
      bump = () => setN((x) => x + 1);
      return (
        <div data-tick={n}>
          <ReviewDocsColumn docs={docs} sessionId="s1" />
        </div>
      );
    }

    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
    await act(async () => {
      root!.render(<Parent />);
    });

    expect(container.textContent).toContain("plan");
    const afterLoad = renders.mock.calls.length;
    expect(afterLoad).toBeGreaterThan(0);

    // Ten unrelated parent re-renders — a card left open for ~20s.
    for (let i = 0; i < 10; i += 1) {
      await act(async () => {
        bump!();
      });
    }

    expect(renders.mock.calls.length).toBe(afterLoad);
  });
});
