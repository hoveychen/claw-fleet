import { describe, expect, it } from "vitest";
import { isMermaidPre } from "./mermaidPre";

/** react-markdown 传给 `pre` 组件的 hast 节点形状（v10）。 */
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
  it("认出 ```mermaid fence（className 是数组，react-markdown 的常规形状）", () => {
    expect(isMermaidPre(pre(["language-mermaid"]))).toBe(true);
  });

  it("className 是字符串时也认", () => {
    expect(isMermaidPre(pre("language-mermaid hljs"))).toBe(true);
  });

  it("别的语言不认", () => {
    expect(isMermaidPre(pre(["language-ts"]))).toBe(false);
    expect(isMermaidPre(pre())).toBe(false);
  });

  it("语言前缀相似的不误伤", () => {
    expect(isMermaidPre(pre(["language-mermaidx"]))).toBe(false);
  });

  it("不是 code 子元素、或节点缺失时一律 false", () => {
    expect(isMermaidPre({ children: [{ tagName: "span", properties: {} }] })).toBe(
      false,
    );
    expect(isMermaidPre({ children: [] })).toBe(false);
    expect(isMermaidPre(undefined)).toBe(false);
    expect(isMermaidPre(null)).toBe(false);
  });
});
