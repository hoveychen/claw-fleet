// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import "../i18n";
import { DshTokenView } from "./DshTokenPanel";
import type { DshSessionCost, DshTokenBreakdown } from "../types";

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

function render(data: DshTokenBreakdown, cost?: DshSessionCost | null) {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<DshTokenView data={data} cost={cost} />));
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

  it("shows the provider's real charge, not a figure derived from the tokens", () => {
    const cost: DshSessionCost = {
      totalUsd: 0.0342,
      pricedCalls: 3,
      tablePricedCalls: 0,
      unpricedCalls: 0,
      unpriceableCalls: 0,
      note: "",
    };
    const text = render(live, cost).textContent ?? "";
    expect(text).toContain("$0.0342");
  });

  it("renders no spend and keeps the tokens when the cost lookup came back empty", () => {
    const cost: DshSessionCost = {
      totalUsd: null,
      pricedCalls: 0,
      tablePricedCalls: 0,
      unpricedCalls: 3,
      unpriceableCalls: 0,
      note: "no OpenRouter API key configured",
    };
    const text = render(live, cost).textContent ?? "";
    // Never $0.00 — that would read as "this session was free".
    expect(text).not.toContain("$0");
    expect(text).toContain("no OpenRouter API key configured");
    expect(text).toContain("71.2k");
  });

  it("keeps a sub-cent total legible instead of rounding it to zero", () => {
    const cost: DshSessionCost = {
      totalUsd: 0.0004,
      pricedCalls: 1,
      tablePricedCalls: 0,
      unpricedCalls: 0,
      unpriceableCalls: 0,
      note: "",
    };
    const text = render(live, cost).textContent ?? "";
    // Four decimals, not "<$0.01": the panel already obtained the figure, and a
    // DeepSeek-official turn genuinely lands in this range.
    expect(text).toContain("$0.0004");
    expect(text).not.toContain("$0.00 ");
  });

  it("falls back to a bound only below a hundredth of a cent", () => {
    const cost: DshSessionCost = {
      totalUsd: 0.00002,
      pricedCalls: 1,
      tablePricedCalls: 1,
      unpricedCalls: 0,
      unpriceableCalls: 0,
      note: "",
    };
    const text = render(live, cost).textContent ?? "";
    expect(text).toContain("<$0.0001");
    // Never a bare zero — that would read as "this call was free".
    expect(text).not.toContain("$0.0000");
  });

  it("says a table-priced total came from published rates, not from the provider", () => {
    const cost: DshSessionCost = {
      totalUsd: 0.00284702,
      pricedCalls: 1,
      tablePricedCalls: 1,
      unpricedCalls: 0,
      unpriceableCalls: 0,
      note: "1 call(s) priced from DeepSeek's published rates (0 peak / 1 off-peak)",
    };
    const text = render(live, cost).textContent ?? "";
    expect(text).toContain("$0.0028");
    expect(text).toContain("published rates");
    // The receipt wording would be a small lie about where the number came from.
    expect(text).not.toContain("priced by the provider");
  });

  // Caught on screen: against a probe built before `tablePricedCalls` existed,
  // subtracting the absent field rendered a literal "NaN by the provider".
  // RemoteBackend talks to whatever `fleet serve` runs on the other machine, so
  // an older field set is a normal state, not a corrupt one.
  it("degrades to the receipt wording when an older backend omits the new field", () => {
    const cost = {
      totalUsd: 0.0342,
      pricedCalls: 3,
      unpricedCalls: 0,
      unpriceableCalls: 0,
      note: "",
    } as unknown as DshSessionCost;
    const text = render(live, cost).textContent ?? "";
    expect(text).not.toContain("NaN");
    expect(text).toContain("3");
    expect(text).toContain("provider");
  });

  it("splits the two kinds of figure when a session used both routes", () => {
    const cost: DshSessionCost = {
      totalUsd: 0.31,
      pricedCalls: 5,
      tablePricedCalls: 3,
      unpricedCalls: 0,
      unpriceableCalls: 0,
      note: "3 call(s) priced from DeepSeek's published rates (1 peak / 2 off-peak)",
    };
    const text = render(live, cost).textContent ?? "";
    expect(text).toContain("2");
    expect(text).toContain("published rates");
    expect(text).toContain("provider");
  });

  it("surfaces calls that were left out of the total", () => {
    const cost: DshSessionCost = {
      totalUsd: 0.02,
      pricedCalls: 2,
      tablePricedCalls: 0,
      unpricedCalls: 0,
      unpriceableCalls: 1,
      note: "1 call(s) went through a provider with no cost API and are not included",
    };
    const text = render(live, cost).textContent ?? "";
    expect(text).toContain("$0.0200");
    // A partial total that does not say it is partial is a wrong total.
    expect(text).toContain("no cost API");
  });
});
