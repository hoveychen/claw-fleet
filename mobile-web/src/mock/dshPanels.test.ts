import { describe, expect, it } from "vitest";

import { tokenRequestFor } from "../agentSource";
import { MOCK_SESSIONS } from "./data";
import { MockRelayClient } from "./relay";

/** The mock relay is the only way to look at the dsh Token tab without a live
 *  desktop: dsh keeps no transcript file, so both of that tab's reads are RPC.
 *  Before this fixture existed the tab was blank in every mock build, which is
 *  the state a screenshot or a design pass would have been taken in.
 *
 *  These assertions are deliberately about *reachability*, not about the
 *  numbers: the point is that the roster has a dsh session at all, that the
 *  panel's own method-picking routes it to the RPC pair, and that both of those
 *  methods answer. A fixture that silently stopped being served would put the
 *  tab back to blank with nothing to notice it. */
describe("mock relay serves the dsh Token tab", () => {
  const client = () => new MockRelayClient({});

  const dshSession = () => {
    const s = MOCK_SESSIONS.find((x) => x.agentSource === "dsh");
    expect(s, "the mock roster must contain a dsh session").toBeDefined();
    return s!;
  };

  it("the roster carries a dsh session addressed by a dsh:// URI", () => {
    const s = dshSession();
    expect(s.jsonlPath).toMatch(/^dsh:\/\//);
  });

  it("routes that session's token panel to the dsh RPC, and it answers", async () => {
    const s = dshSession();
    const req = tokenRequestFor(s);
    expect(req.method).toBe("dsh_token_breakdown");

    const tokens = await client().request<{
      uncachedInputTokens: number;
      cacheReadTokens: number;
      totalTokens: number;
      contextPercent: number;
    }>(req.method, req.params);
    // dsh meters cache reads separately from input that missed the cache;
    // folding them together would overstate a cache-heavy session ~30×.
    expect(tokens.cacheReadTokens).toBeGreaterThan(tokens.uncachedInputTokens);
    expect(tokens.totalTokens).toBeGreaterThan(0);
    expect(tokens.contextPercent).toBeGreaterThan(0);
  });

  it("answers the session's cost, gap included", async () => {
    const s = dshSession();
    const cost = await client().request<{
      totalUsd: number | null;
      pricedCalls: number;
      unpricedCalls: number;
      unpriceableCalls: number;
      note: string;
    }>("dsh_session_cost", { uri: s.jsonlPath });

    expect(cost.totalUsd).toBeGreaterThan(0);
    expect(cost.pricedCalls).toBeGreaterThan(0);
    // The fixture is deliberately partial: a panel that renders it must have
    // something to say about the calls it could not price, rather than folding
    // them into the total as $0.
    expect(cost.unpricedCalls + cost.unpriceableCalls).toBeGreaterThan(0);
    expect(cost.note).not.toBe("");
  });
});
