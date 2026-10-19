import { describe, expect, it } from "vitest";
import type { PendingDecision } from "../types";
import {
  allQuestionsAnswered,
  dockAction,
  fromSubtask,
  isRecommendedOption,
  primaryOptionIndex,
  stripRecommendedSuffix,
  type DockKey,
} from "./dockKeys";

function ask(over: Record<string, unknown> = {}, questions?: unknown[]): PendingDecision {
  return {
    kind: "fleet-ask",
    id: "d1",
    step: 0,
    request: {
      id: "d1",
      sessionId: "s1",
      questions: questions ?? [
        {
          question: "Q1",
          header: "h",
          multiSelect: false,
          options: [
            { label: "Ship it (Recommended)", description: "" },
            { label: "Wait", description: "" },
          ],
        },
      ],
    },
    selections: {},
    customAnswers: {},
    formAnswers: {},
    multiSelectOverrides: {},
    attachments: {},
    ...over,
  } as unknown as PendingDecision;
}

function key(k: string, over: Partial<DockKey> = {}): DockKey {
  return { key: k, metaKey: false, ctrlKey: false, altKey: false, inEditable: false, ...over };
}

describe("recommended option", () => {
  it("recognises the suffixes agents use and strips them for display", () => {
    expect(isRecommendedOption("Ship it (Recommended)")).toBe(true);
    expect(isRecommendedOption("发布（推荐）")).toBe(true);
    expect(isRecommendedOption("发布 (推荐)")).toBe(true);
    expect(isRecommendedOption("Recommended reading")).toBe(false);
    expect(stripRecommendedSuffix("Ship it (Recommended)")).toBe("Ship it");
  });

  it("makes the recommended option primary, else the first single-select one", () => {
    expect(primaryOptionIndex(["a", "b (Recommended)"], false)).toBe(1);
    expect(primaryOptionIndex(["a", "b（推荐）"], true)).toBe(1);
    expect(primaryOptionIndex(["a", "b"], false)).toBe(0);
    expect(primaryOptionIndex(["a", "b"], true)).toBe(-1);
    expect(primaryOptionIndex([], false)).toBe(-1);
  });
});

describe("fromSubtask", () => {
  it("is true only when the card names a sub-task", () => {
    expect(fromSubtask({ task: { subtaskId: "sub" } })).toBe(true);
    expect(fromSubtask({ task: { subtaskId: null } })).toBe(false);
    expect(fromSubtask({})).toBe(false);
  });
});

describe("dockAction", () => {
  it("maps a digit to the option of the current question", () => {
    expect(dockAction(ask(), key("2"))).toEqual({
      kind: "pick",
      question: "Q1",
      label: "Wait",
      multi: false,
    });
    expect(dockAction(ask(), key("3"))).toBeNull();
  });

  it("leaves digits to a text field that has focus", () => {
    expect(dockAction(ask(), key("1", { inEditable: true }))).toBeNull();
  });

  it("sends with ⌘↵ only once the card is answered, even from a text field", () => {
    expect(dockAction(ask(), key("Enter", { metaKey: true }))).toBeNull();
    const answered = ask({ customAnswers: { Q1: "do it" } });
    expect(allQuestionsAnswered(answered as never)).toBe(true);
    expect(dockAction(answered, key("Enter", { metaKey: true, inEditable: true }))).toEqual({
      kind: "submit",
    });
  });

  it("advances a multi-question card before sending", () => {
    const two = ask({ selections: { Q1: ["Wait"] } }, [
      { question: "Q1", multiSelect: false, options: [{ label: "Wait", description: "" }] },
      { question: "Q2", multiSelect: false, options: [{ label: "Go", description: "" }] },
    ]);
    expect(dockAction(two, key("Enter", { ctrlKey: true }))).toEqual({ kind: "next" });
  });

  it("holds a required form field against sending", () => {
    const form = ask({}, [
      {
        question: "Q1",
        multiSelect: false,
        options: [],
        formFields: [{ name: "n", label: "Name", kind: "text", required: true }],
      },
    ]);
    expect(dockAction(form, key("Enter", { metaKey: true }))).toBeNull();
    expect(
      dockAction({ ...form, formAnswers: { n: "x" } } as PendingDecision, key("Enter", { metaKey: true })),
    ).toEqual({ kind: "submit" });
  });

  it("denies a guard on Esc and ignores Esc on a question", () => {
    const guard = { kind: "guard", id: "g", request: {} } as unknown as PendingDecision;
    expect(dockAction(guard, key("Escape"))).toEqual({ kind: "deny" });
    expect(dockAction(ask(), key("Escape"))).toBeNull();
  });
});
