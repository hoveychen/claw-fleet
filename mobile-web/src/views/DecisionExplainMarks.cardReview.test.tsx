/**
 * The phone's card review buttons, as markup. The press → preselect → confirm
 * flow is pinned by the desktop's DecisionPanel.cardReview.test.tsx (this
 * package has no DOM environment); here the buttons are only checked to be
 * plain buttons that name what they ask about.
 */
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { FlipConditionsButton, SteelmanButton } from "./DecisionExplainMarks";
import { t } from "../i18n";

describe("card review buttons", () => {
  it("names the option a steelman is about, as a side button or a link", () => {
    const side = renderToStaticMarkup(<SteelmanButton label="直接全量" variant="side" onPress={vi.fn()} />);
    expect(side).toContain('type="button"');
    expect(side).toContain("直接全量");
    expect(side).toContain(t("反方"));
    const link = renderToStaticMarkup(<SteelmanButton label="直接全量" variant="link" onPress={vi.fn()} />);
    expect(link).toContain(t("反方论证"));
  });

  it("offers the flip question under the question", () => {
    const html = renderToStaticMarkup(<FlipConditionsButton onPress={vi.fn()} />);
    expect(html).toContain('data-testid="card-flip-btn"');
    expect(html).toContain(t("翻转条件"));
  });
});
