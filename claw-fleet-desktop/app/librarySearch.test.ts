// Snippet highlighting for 资料 search hits (shared-ts/librarySearch.ts).

import { describe, expect, it } from "vitest";
import { snippetRuns } from "../../shared-ts/librarySearch";

describe("snippetRuns", () => {
  it("splits marked and plain runs", () => {
    expect(snippetRuns("…前文<mark>索引</mark>后文<mark>x</mark>")).toEqual([
      { text: "…前文", mark: false },
      { text: "索引", mark: true },
      { text: "后文", mark: false },
      { text: "x", mark: true },
    ]);
  });

  it("keeps raw html as plain text and tolerates unbalanced markers", () => {
    expect(snippetRuns("<b>a</b> <mark>b")).toEqual([{ text: "<b>a</b> <mark>b", mark: false }]);
    expect(snippetRuns("")).toEqual([]);
  });
});
