// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import "../i18n";
import { DshTokenView } from "./DshTokenPanel";
import type { DshTokenBreakdown } from "../types";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

/** Verbatim projections of a live four-turn dsh session, as mapped by
 *  `dsh_source::dsh_token_breakdown_from_projections`. */
const live: DshTokenBreakdown = {
  uncachedInputTokens: 36,
  cacheReadTokens: 60325,
  cacheWriteTokens: 10306,
  outputTokens: 509,
  totalTokens: 71176,
  systemTokens: 1506,
  toolsTokens: 6376,
  messageTokens: 574,
  projectedTokens: 9215,
  contextWindow: 200000,
  contextPercent: 9215 / 200000,
};

function render(data: DshTokenBreakdown) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<DshTokenView data={data} />));
  return container!;
}

describe("DshTokenPanel", () => {
  it("keeps cumulative billing and current context as two separate readings", () => {
    const el = render(live);
    const text = el.textContent ?? "";

    // Billed is the whole-session total: 36 + 60325 + 10306 + 509.
    expect(text).toContain("71.2k");
    // Context is a snapshot of the window — 9215 of 200k, NOT 71k of 200k.
    // Summing the two readings is the mistake this panel exists to prevent.
    expect(text).toContain("5%");
    expect(text).toContain("9.2k / 200.0k");
    expect(text).not.toContain("36%");
  });

  it("renders all four billed buckets, cache read included", () => {
    const el = render(live);
    const text = el.textContent ?? "";
    // 60325 cache-read tokens are 85% of the session's spend; a panel that
    // dropped them (as the Claude/Codex file-readers do for dsh) would show
    // a near-empty tab.
    expect(text).toContain("60.3k");
    expect(text).toContain("10.3k");
    expect(text).toContain("509");
  });

  it("shows an em dash rather than 0% when dsh has no context window yet", () => {
    const blank: DshTokenBreakdown = {
      uncachedInputTokens: 0,
      cacheReadTokens: 0,
      cacheWriteTokens: 0,
      outputTokens: 0,
      totalTokens: 0,
      systemTokens: 0,
      toolsTokens: 0,
      messageTokens: 0,
      projectedTokens: null,
      contextWindow: null,
      contextPercent: null,
    };
    const text = render(blank).textContent ?? "";
    expect(text).toContain("—");
    expect(text).not.toContain("100%");
  });
});
