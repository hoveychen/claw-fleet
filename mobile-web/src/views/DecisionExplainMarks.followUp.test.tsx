/**
 * The phone's card side questions, as markup.
 *
 * A follow-up is its own record threading off the one it continues (the fork
 * is never resumed), so a flat list showed a two-turn conversation as two
 * unrelated answers. This suite pins the grouping and when the follow-up box
 * is offered; the submit path itself is the desktop's
 * DecisionExplainColumn.followUp.test.tsx, which has a DOM.
 */
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { DecisionExplainAnswers } from "./DecisionExplainMarks";
import type { ExplainRecord } from "../sessionExplain";

function rec(id: string, thread: string[], status: ExplainRecord["status"], createdMs: number): ExplainRecord {
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
    dismissed: false,
  };
}

function markup(answers: ExplainRecord[], busy = false) {
  return renderToStaticMarkup(
    <DecisionExplainAnswers
      answers={answers}
      busy={busy}
      onDismiss={vi.fn()}
      onFollowUp={vi.fn()}
    />,
  );
}

const forms = (html: string) => html.split("<form").length - 1;

describe("DecisionExplainAnswers", () => {
  it("draws one card with one follow-up box per chain", () => {
    const html = markup([rec("a", [], "done", 1), rec("b", ["a"], "done", 2), rec("x", [], "done", 5)]);
    // Two chains ('a'→'b' and 'x'), not three loose records.
    expect(forms(html)).toBe(2);
    // Both turns of the chain are shown, oldest first.
    expect(html.indexOf("a-a")).toBeLessThan(html.indexOf("a-b"));
  });

  it("quotes the passage once per chain, not once per turn", () => {
    const html = markup([rec("a", [], "done", 1), rec("b", ["a"], "done", 2)]);
    expect(html.split("灰度到 5%").length - 1).toBe(1);
  });

  it("withholds the box until the chain's last answer has settled", () => {
    expect(forms(markup([rec("a", [], "done", 1), rec("b", ["a"], "running", 2)]))).toBe(0);
  });

  it("disables sending while an ask is in flight", () => {
    expect(markup([rec("a", [], "done", 1)], true)).toContain("disabled");
  });

  it("renders nothing at all with no answers", () => {
    expect(markup([])).toBe("");
  });
});
