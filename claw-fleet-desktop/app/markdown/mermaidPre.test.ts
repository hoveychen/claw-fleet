import { describe, expect, it } from "vitest";
import { isMermaidPre } from "./mermaidPre";

/** hast node shape passed by react-markdown to the `pre` component (v10). */
function pre(codeClass?: string | string[]) {
  return {
    type: "element",
    tagName: "pre",
    children: [
      {
        type: "element",
        tagName: "code",
        properties: codeClass === undefined ? {} : { className: codeClass },
        children: [{ type: "text", value: "flowchart TB\n  A --> B" }],
      },
    ],
  };
}

describe("isMermaidPre", () => {
  it("recognizes ```mermaid fence (className is array, react-markdown's regular shape)", () => {
    expect(isMermaidPre(pre(["language-mermaid"]))).toBe(true);
  });

  it("also recognizes when className is a string", () => {
    expect(isMermaidPre(pre("language-mermaid hljs"))).toBe(true);
  });

  it("does not recognize other languages", () => {
    expect(isMermaidPre(pre(["language-ts"]))).toBe(false);
    expect(isMermaidPre(pre())).toBe(false);
  });

  it("does not false-match similar language prefixes", () => {
    expect(isMermaidPre(pre(["language-mermaidx"]))).toBe(false);
  });

  it("returns false when not a code child element or when node is missing", () => {
    expect(isMermaidPre({ children: [{ tagName: "span", properties: {} }] })).toBe(
      false,
    );
    expect(isMermaidPre({ children: [] })).toBe(false);
    expect(isMermaidPre(undefined)).toBe(false);
    expect(isMermaidPre(null)).toBe(false);
  });
});
