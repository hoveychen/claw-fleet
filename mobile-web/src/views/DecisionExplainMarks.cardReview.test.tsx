/**
 * The phone's card review buttons and answer headings, as markup (this package
 * has no DOM environment; the press-sends-at-once flow is pinned by the
 * desktop's DecisionPanel.cardReview.test.tsx, both sides share the quote and
 * running-check helpers from shared-ts).
 */
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { steelmanQuote } from "../../../shared-ts/sessionExplain";
import { t } from "../i18n";
import type { ExplainRecord } from "../sessionExplain";
import { DecisionExplainAnswers, FlipConditionsButton, SteelmanButton, type DecisionExplain } from "./DecisionExplainMarks";

const QUESTION = "先灰度还是直接全量？";
const OPTION = { label: "直接全量", description: "跳过灰度" };

function explain(answers: Partial<ExplainRecord>[] = []): DecisionExplain {
  return {
    enabled: true,
    busy: false,
    ask: vi.fn(),
    followUp: vi.fn(),
    answers: answers as ExplainRecord[],
    dismiss: vi.fn(),
  };
}

function record(over: Partial<ExplainRecord>): Partial<ExplainRecord> {
  return { id: "r1", sessionId: "s", preset: "steelman", quote: "", question: "", status: "running", text: "", thread: [], ...over };
}

describe("card review buttons", () => {
  it("names the option a steelman is about, as a side button or a link", () => {
    const side = renderToStaticMarkup(
      <SteelmanButton explain={explain()} question={QUESTION} option={OPTION} msgIdx={0} variant="side" />,
    );
    expect(side).toContain('type="button"');
    expect(side).toContain("直接全量");
    expect(side).toContain(t("反方"));
    expect(side).not.toContain("disabled");
    const link = renderToStaticMarkup(
      <SteelmanButton explain={explain()} question={QUESTION} option={OPTION} msgIdx={0} variant="link" />,
    );
    expect(link).toContain(t("反方论证"));
  });

  it("waits while its own ask is being answered", () => {
    const busy = explain([record({ quote: steelmanQuote(QUESTION, OPTION) })]);
    const html = renderToStaticMarkup(
      <SteelmanButton explain={busy} question={QUESTION} option={OPTION} msgIdx={0} variant="side" />,
    );
    expect(html).toContain("disabled");
    expect(html).toContain('aria-busy="true"');
    const other = renderToStaticMarkup(
      <SteelmanButton explain={busy} question={QUESTION} option={{ label: "照做" }} msgIdx={0} variant="side" />,
    );
    expect(other).not.toContain("disabled");
  });

  it("offers the flip question under the question", () => {
    const html = renderToStaticMarkup(<FlipConditionsButton explain={explain()} question={QUESTION} msgIdx={0} />);
    expect(html).toContain('data-testid="card-flip-btn"');
    expect(html).toContain(t("翻转条件"));
  });
});

describe("card review answers", () => {
  it("are headed by what was asked, not the raw quote", () => {
    const html = renderToStaticMarkup(
      <DecisionExplainAnswers
        answers={[record({ quote: steelmanQuote(QUESTION, OPTION) }) as ExplainRecord]}
        busy={false}
        onDismiss={vi.fn()}
        onFollowUp={vi.fn()}
      />,
    );
    expect(html).toContain(`${t("反方论证")} · 直接全量`);
    expect(html).not.toContain(QUESTION);
  });
});
