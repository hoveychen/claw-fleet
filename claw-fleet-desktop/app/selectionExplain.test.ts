// @vitest-environment jsdom
import { describe, expect, it } from "vitest";

import {
  cacheHitRatio,
  costLabel,
  quoteSnippet,
  readAssistantSelection,
  selectQuoteIn,
} from "./selectionExplain";

function transcript(): HTMLElement {
  const root = document.createElement("div");
  root.innerHTML = `
    <div data-msg-idx="0" data-role="user"><p id="u">please explain the cache</p></div>
    <div data-msg-idx="1" data-role="assistant" data-msg-uuid="aaa">
      <p id="a1">The <code>prompt</code> cache stores the prefix.</p>
      <p id="a2">A miss costs the full write.</p>
    </div>
    <div data-msg-idx="2" data-role="assistant"><p id="b1">Second turn.</p></div>
  `;
  document.body.appendChild(root);
  return root;
}

function select(startNode: Node, startOff: number, endNode: Node, endOff: number): Selection {
  const range = document.createRange();
  range.setStart(startNode, startOff);
  range.setEnd(endNode, endOff);
  const sel = window.getSelection()!;
  sel.removeAllRanges();
  sel.addRange(range);
  return sel;
}

describe("readAssistantSelection", () => {
  it("returns the quote and the row's anchor for a selection inside one assistant row", () => {
    const root = transcript();
    const a2 = root.querySelector("#a2")!.firstChild!;
    const got = readAssistantSelection(root, select(a2, 2, a2, 12));
    expect(got).not.toBeNull();
    expect(got!.quote).toBe("miss costs");
    expect(got!.msgIdx).toBe(1);
    expect(got!.msgUuid).toBe("aaa");
    root.remove();
  });

  it("spans inline markup within the row", () => {
    const root = transcript();
    const a1 = root.querySelector("#a1")!;
    const got = readAssistantSelection(
      root,
      select(a1.firstChild!, 0, a1.lastChild!, 6),
    );
    expect(got?.quote).toBe("The prompt cache");
    root.remove();
  });

  it("rejects user prose, cross-row drags, collapsed and outside selections", () => {
    const root = transcript();
    const u = root.querySelector("#u")!.firstChild!;
    expect(readAssistantSelection(root, select(u, 0, u, 6))).toBeNull();
    const a2 = root.querySelector("#a2")!.firstChild!;
    const b1 = root.querySelector("#b1")!.firstChild!;
    expect(readAssistantSelection(root, select(a2, 0, b1, 3))).toBeNull();
    expect(readAssistantSelection(root, select(a2, 3, a2, 3))).toBeNull();
    const outside = document.createElement("p");
    outside.textContent = "elsewhere";
    document.body.appendChild(outside);
    expect(readAssistantSelection(root, select(outside.firstChild!, 0, outside.firstChild!, 4))).toBeNull();
    // One glyph is a mis-drag.
    expect(readAssistantSelection(root, select(a2, 0, a2, 1))).toBeNull();
    outside.remove();
    root.remove();
  });
});

describe("selectQuoteIn", () => {
  it("re-selects a quote that markdown split across nodes, folding whitespace", () => {
    const root = transcript();
    const row = root.querySelector("[data-msg-idx='1']") as HTMLElement;
    expect(selectQuoteIn(row, "prompt  cache\nstores")).toBe(true);
    expect(window.getSelection()!.toString().replace(/\s+/g, " ")).toBe("prompt cache stores");
    expect(selectQuoteIn(row, "not in this row")).toBe(false);
    root.remove();
  });
});

describe("labels", () => {
  it("snips the first line to chip length", () => {
    expect(quoteSnippet("short")).toBe("short");
    expect(quoteSnippet("first line\nsecond")).toBe("first line");
    expect(quoteSnippet("x".repeat(60), 10)).toBe("xxxxxxxxx…");
  });

  it("computes the cache share and formats spend", () => {
    expect(cacheHitRatio({ inputTokens: 0, cacheReadTokens: 0, cacheCreationTokens: 0 })).toBeNull();
    expect(cacheHitRatio({ inputTokens: 10, cacheReadTokens: 90, cacheCreationTokens: 0 })).toBe(0.9);
    expect(costLabel(null)).toBe("");
    expect(costLabel(0)).toBe("$0");
    expect(costLabel(0.004)).toBe("<$0.01");
    expect(costLabel(0.073)).toBe("$0.07");
    expect(costLabel(1.61)).toBe("$1.6");
  });
});
