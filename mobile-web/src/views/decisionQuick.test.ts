import { describe, expect, it } from "vitest";
import { isQuickAnswer, recommendedIndex } from "./DecisionsView";

describe("recommendedIndex", () => {
  it("picks the option carrying a recommended mark", () => {
    expect(
      recommendedIndex([{ label: "Split in two" }, { label: "Do it all at once (Recommended)" }]),
    ).toBe(1);
    expect(recommendedIndex([{ label: "先只改结账页" }, { label: "一次性全部改（推荐）" }])).toBe(1);
  });

  it("falls back to the first option", () => {
    expect(recommendedIndex([{ label: "a" }, { label: "b" }])).toBe(0);
  });
});

describe("isQuickAnswer", () => {
  const opt = [{ label: "a" }];
  it("is a single single-select question with options and no form", () => {
    expect(isQuickAnswer([{ options: opt }])).toBe(true);
  });

  it("is not quick for multi-select, forms, option-less or several questions", () => {
    expect(isQuickAnswer([{ options: opt, multiSelect: true }])).toBe(false);
    expect(isQuickAnswer([{ options: opt, formFields: [{}] }])).toBe(false);
    expect(isQuickAnswer([{ options: [] }])).toBe(false);
    expect(isQuickAnswer([{ options: opt }, { options: opt }])).toBe(false);
  });
});
