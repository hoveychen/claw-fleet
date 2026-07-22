import { describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";

// The column fetches bodies lazily; keep the promise pending so the static
// render shows the initial (tab bar + loading) state deterministically.
vi.mock("@tauri-apps/api/core", () => ({
  invoke: vi.fn(() => new Promise(() => {})),
}));
vi.mock("react-i18next", () => ({
  useTranslation: () => ({ t: (_k: string, d: string) => d }),
}));
vi.mock("../hooks/usePathLinks", () => ({ usePathMarkdown: () => ({}) }));
vi.mock("./AutoHeightFrame", () => ({ AutoHeightFrame: () => null }));

import { ReviewDocsColumn } from "./ReviewDocsColumn";
import type { ReviewDoc } from "../types";

const docs: ReviewDoc[] = [
  { kind: "file", ref: "/Users/x/proj/docs/design.md", title: null },
  { kind: "wiki", ref: "arch/overview", title: "Overview" },
];

describe("ReviewDocsColumn", () => {
  it("renders one tab per attached doc", () => {
    const html = renderToStaticMarkup(
      <ReviewDocsColumn docs={docs} sessionId="s1" />,
    );
    // File tab falls back to the base file name; wiki tab uses the given title.
    expect(html).toContain("design.md");
    expect(html).toContain("Overview");
    // Two tablist buttons.
    expect(html.match(/role="tab"/g)?.length).toBe(2);
  });

  it("shows the loading state before a body resolves", () => {
    const html = renderToStaticMarkup(
      <ReviewDocsColumn docs={docs} sessionId="s1" />,
    );
    expect(html).toContain("Loading…");
  });

  it("renders nothing when there are no docs", () => {
    const html = renderToStaticMarkup(
      <ReviewDocsColumn docs={[]} sessionId="s1" />,
    );
    expect(html).toBe("");
  });
});
