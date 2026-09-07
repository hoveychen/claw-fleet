import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it } from "vitest";
import { railToolIcon } from "./Rail";

describe("railToolIcon", () => {
  it("renders TaskOutput as a waiting step instead of an unknown-tool wrench", () => {
    const markup = renderToStaticMarkup(railToolIcon("TaskOutput"));

    expect(markup).toContain("lucide-clock-3");
    expect(markup).not.toContain("lucide-wrench");
  });
});
