import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { CompactSummaryRule } from "./SessionDetailView";

/** A compaction boundary is a one-line rule, never the summary as a bubble. */
describe("CompactSummaryRule", () => {
  it("renders a folded rule that hides the summary until tapped", () => {
    const html = renderToStaticMarkup(<CompactSummaryRule summary={"## 交接摘要\n\nlong body"} />);
    expect(html).toContain('data-testid="compact-rule"');
    expect(html).toContain('aria-expanded="false"');
    expect(html).not.toContain("long body");
  });

  it("drops the disclosure arrow when there is no summary to show", () => {
    const html = renderToStaticMarkup(<CompactSummaryRule summary="" />);
    expect(html).not.toContain("aria-expanded");
    expect(html).not.toContain("▸");
  });
});
