/**
 * The phone's 追问 pane, as markup.
 *
 * One card per *chain*, not per record: a follow-up is its own record (the
 * fork is never resumed), so the flat list this replaced showed a two-turn
 * conversation as two unrelated cards, the newer one above the question it
 * answered. The grouping itself is pinned in explainThreads.test.ts (desktop);
 * what matters here is that the card summarises the chain — opening question,
 * latest state — and offers exactly one follow-up box.
 */
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { SessionExplainsTab } from "./SessionExplainsTab";
import { t } from "../i18n";
import type { ExplainRecord } from "../sessionExplain";

function rec(
  id: string,
  thread: string[],
  status: ExplainRecord["status"],
  createdMs: number,
  over: Partial<ExplainRecord> = {},
): ExplainRecord {
  return {
    id,
    sessionId: "s-1",
    source: "claude-code",
    createdMs,
    updatedMs: createdMs,
    preset: thread.length > 0 ? "custom" : "explain",
    quote: "灰度到 5%",
    question: `q-${id}`,
    thread,
    status,
    text: status === "done" ? `a-${id}` : "",
    inputTokens: 0,
    outputTokens: 0,
    cacheReadTokens: 0,
    cacheCreationTokens: 0,
    durationMs: 0,
    ...over,
  };
}

function markup(explains: ExplainRecord[], openId: string | null, busy = false) {
  return renderToStaticMarkup(
    <SessionExplainsTab
      explains={explains}
      loaded
      openId={openId}
      busy={busy}
      onToggle={vi.fn()}
      onLocate={vi.fn()}
      onFollowUp={vi.fn()}
    />,
  );
}

const cards = (html: string) => html.split('data-testid="explain-card"').length - 1;
const forms = (html: string) => html.split("<form").length - 1;

const CHAIN = [rec("a", [], "done", 1), rec("b", ["a"], "done", 2)];

describe("SessionExplainsTab", () => {
  it("draws one card for a chain, however many turns it has", () => {
    expect(cards(markup(CHAIN, "a"))).toBe(1);
    expect(cards(markup([...CHAIN, rec("x", [], "done", 9)], "a"))).toBe(2);
  });

  it("stays open when a follow-up reports its own new id", () => {
    // SessionDetailView sets openExplain to the *new* record's id; the chain it
    // continues must not collapse under it.
    const html = markup(CHAIN, "b");
    expect(html).toContain("a-a");
    expect(html).toContain("a-b");
  });

  it("shows both turns in asking order under one quote", () => {
    const html = markup(CHAIN, "a");
    expect(html.indexOf("a-a")).toBeLessThan(html.indexOf("a-b"));
    expect(html.split("灰度到 5%").length - 1).toBe(1);
  });

  it("offers exactly one follow-up box, at the foot of the chain", () => {
    expect(forms(markup(CHAIN, "a"))).toBe(1);
  });

  it("withholds the box while the chain's latest turn is still forking", () => {
    expect(forms(markup([rec("a", [], "done", 1), rec("b", ["a"], "running", 2)], "a"))).toBe(0);
  });

  it("heads a multi-turn chain with its opening question and a turn count", () => {
    const html = markup(CHAIN, null);
    expect(html).toContain("q-a");
    expect(html).toContain(t("{0} 轮", 2));
  });

  it("previews the chain's latest answer while collapsed", () => {
    const html = markup(CHAIN, null);
    expect(html).toContain("a-b");
    expect(forms(html)).toBe(0);
  });

  it("reports the latest turn's failure in the head, not the first turn's success", () => {
    const html = markup([rec("a", [], "done", 1), rec("b", ["a"], "error", 2, { error: "boom" })], null);
    expect(html).toContain(t("失败"));
    expect(html).toContain("boom");
  });
});
