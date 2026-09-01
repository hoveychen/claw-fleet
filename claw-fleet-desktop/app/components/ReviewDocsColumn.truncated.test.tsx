// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it, vi } from "vitest";

const invoke = vi.fn();
vi.mock("@tauri-apps/api/core", () => ({ invoke: (...a: unknown[]) => invoke(...a) }));
vi.mock("./AutoHeightFrame", () => ({ AutoHeightFrame: () => null }));

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
});

const docs: ReviewDoc[] = [{ kind: "file", ref: "/Users/x/proj/TASKS.md", title: null }];

async function render(content: ReviewDocContent) {
  invoke.mockResolvedValue(content);
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  await act(async () => {
    root!.render(<ReviewDocsColumn docs={docs} sessionId="s1" />);
  });
  return container!;
}

describe("ReviewDocsColumn truncation banner", () => {
  it("tells the user when the backend only sent the head of the doc", async () => {
    // What a months-old TASKS.md looks like after `clip_oversized_markdown`.
    const el = await render({
      format: "markdown",
      body: "- [ ] **P1** — 任务\n",
      title: "TASKS.md",
      truncated: { shownLines: 2000, totalLines: 16635, totalBytes: 1_558_456 },
    });
    // Silently showing a partial doc is the failure this guards: the counts and
    // the real size must be on screen, whichever locale is active.
    const text = el.textContent ?? "";
    expect(text).toContain("2000");
    expect(text).toContain("16635");
    expect(text).toContain("1.5 MB");
    // The body itself still renders.
    expect(text).toContain("任务");
  });

  it("shows no banner for a doc that fit within budget", async () => {
    const el = await render({
      format: "markdown",
      body: "# hi\n",
      title: "notes.md",
      truncated: null,
    });
    const text = el.textContent ?? "";
    expect(text).toContain("hi");
    expect(text).not.toContain("1.5 MB");
    expect(text.includes("过大") || text.includes("too large")).toBe(false);
  });
});
