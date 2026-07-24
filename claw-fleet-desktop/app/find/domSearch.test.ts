// @vitest-environment jsdom
import { describe, expect, it } from "vitest";
import { findMatches } from "./domSearch";

function build(html: string): HTMLElement {
  const root = document.createElement("div");
  root.innerHTML = html;
  return root;
}

describe("findMatches", () => {
  it("returns [] for an empty query", () => {
    expect(findMatches(build("<p>hello</p>"), "")).toEqual([]);
  });

  it("finds a single match with correct offsets", () => {
    const root = build("<p>hello world</p>");
    const m = findMatches(root, "world");
    expect(m).toHaveLength(1);
    expect(m[0].start).toBe(6);
    expect(m[0].end).toBe(11);
    expect(m[0].node.data).toBe("hello world");
  });

  it("is case-insensitive", () => {
    const m = findMatches(build("<p>Hello HELLO hello</p>"), "hello");
    expect(m).toHaveLength(3);
  });

  it("finds non-overlapping repeats within one node", () => {
    const m = findMatches(build("<p>aaaa</p>"), "aa");
    expect(m).toHaveLength(2);
    expect(m.map((x) => x.start)).toEqual([0, 2]);
  });

  it("returns matches in document order across nodes", () => {
    const root = build("<p>alpha</p><div><span>beta</span><span>alpha</span></div>");
    const m = findMatches(root, "alpha");
    expect(m).toHaveLength(2);
    expect(m[0].node.data).toBe("alpha");
    expect(m[1].node.parentElement?.tagName).toBe("SPAN");
  });

  it("prunes subtrees the skip predicate rejects", () => {
    const root = build('<p>keep</p><div data-skip="1"><p>keep</p></div>');
    const m = findMatches(root, "keep", (el) => el.hasAttribute("data-skip"));
    expect(m).toHaveLength(1);
    expect(m[0].node.parentElement?.parentElement).toBe(root);
  });

  it("skips script/style content when the predicate says so", () => {
    const root = build("<style>keep{}</style><p>keep</p>");
    const skip = (el: Element) => el.tagName === "STYLE" || el.tagName === "SCRIPT";
    const m = findMatches(root, "keep", skip);
    expect(m).toHaveLength(1);
    expect(m[0].node.parentElement?.tagName).toBe("P");
  });
});
