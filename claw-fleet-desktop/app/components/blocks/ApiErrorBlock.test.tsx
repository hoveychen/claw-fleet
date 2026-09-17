// @vitest-environment jsdom
/**
 * The failed-turn card. What is worth pinning is not the markup but the two
 * promises it makes to the reader: that Claude Code's own wording survives
 * verbatim (it is the only part that names the model / limit / host that
 * failed), and that a button only ever appears when something is wired behind
 * it — a card offering "retry" that does nothing is worse than the grey prose it
 * replaced.
 */
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

// Stands in for i18next, including its `{{var}}` interpolation — the countdown
// arrives through it, so a stub that returned the template would hide whether
// the card ever computed a time at all.
vi.mock("react-i18next", () => ({
  useTranslation: () => ({
    t: (k: string, arg?: unknown) => {
      if (typeof arg === "string") return arg;
      const opts = (arg ?? {}) as Record<string, unknown> & { defaultValue?: string };
      return (opts.defaultValue ?? k).replace(/\{\{(\w+)\}\}/g, (m, name) =>
        name in opts ? String(opts[name]) : m,
      );
    },
  }),
}));

import { classifySyntheticError, type ErrorAction } from "../../../../shared-ts/syntheticError";
import { ApiErrorBlock } from "./ApiErrorBlock";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const auth = classifySyntheticError({
  type: "assistant",
  error: "authentication_failed",
  isApiErrorMessage: true,
  message: {
    model: "<synthetic>",
    content: [
      { type: "text", text: "Failed to authenticate: OAuth session expired and could not be refreshed" },
    ],
  },
})!;

const rateLimited = classifySyntheticError({
  type: "assistant",
  error: "rate_limit",
  isApiErrorMessage: true,
  quotaLimits: { resetsAt: Math.floor(Date.now() / 1000) + 125 },
  message: {
    model: "<synthetic>",
    content: [{ type: "text", text: "You've hit your session limit · resets 3:50am (America/Los_Angeles)" }],
  },
})!;

const safeguards = classifySyntheticError({
  type: "assistant",
  error: "safeguards",
  isApiErrorMessage: true,
  message: { model: "<synthetic>", content: [{ type: "text", text: "API Error: safeguards flagged this" }] },
})!;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root!.unmount());
  container!.remove();
  container = null;
  root = null;
});

type Props = Parameters<typeof ApiErrorBlock>[0];

function draw(props: Props) {
  act(() => root!.render(<ApiErrorBlock {...props} />));
  return container!;
}

const actionsOf = (el: HTMLElement): (string | null)[] =>
  [...el.querySelectorAll("[data-action]")].map((b) => b.getAttribute("data-action"));

describe("ApiErrorBlock", () => {
  it("keeps Claude Code's wording verbatim", () => {
    const el = draw({ info: auth });
    expect(el.textContent).toContain(
      "Failed to authenticate: OAuth session expired and could not be refreshed",
    );
  });

  it("shows no buttons when nothing is wired behind them", () => {
    expect(actionsOf(draw({ info: auth }))).toEqual([]);
  });

  it("offers login before retry once an action handler exists", () => {
    const seen: ErrorAction[] = [];
    const el = draw({ info: auth, onAction: (a) => seen.push(a) });
    expect(actionsOf(el)).toEqual(["login", "retry"]);

    act(() => {
      el.querySelector<HTMLButtonElement>('[data-action="login"]')!.click();
    });
    expect(seen).toEqual(["login"]);
  });

  it("locks every button while one action is running", () => {
    const el = draw({ info: auth, onAction: vi.fn(), busy: "login" });
    for (const b of el.querySelectorAll("[data-action]")) {
      expect((b as HTMLButtonElement).disabled).toBe(true);
    }
  });

  it("counts down from the structured quota, not from the prose", () => {
    // 125s out. The prose in this fixture says "3:50am", a different instant
    // entirely — the point of reading `quotaLimits.resetsAt` is not having to
    // reverse-engineer that string back into a time.
    const el = draw({ info: rateLimited });
    expect(el.querySelector("[data-testid=api-error-countdown]")?.textContent).toMatch(/2:0\d/);
  });

  it("renders no countdown for an error that has no reset to wait for", () => {
    expect(draw({ info: auth }).querySelector("[data-testid=api-error-countdown]")).toBeNull();
  });

  it("holds the retry shut while the quota window is still counting down", () => {
    const el = draw({ info: rateLimited, onAction: vi.fn() });
    const retry = el.querySelector<HTMLButtonElement>('[data-action="retry"]')!;
    const swap = el.querySelector<HTMLButtonElement>('[data-action="switchModel"]')!;
    // Spending a resume to earn the same error back is not a retry, it is a
    // second failure. The sibling model is available right now.
    expect(retry.disabled).toBe(true);
    expect(swap.disabled).toBe(false);
  });

  it("carries the severity so a rate limit can stop looking like a failure", () => {
    const el = draw({ info: rateLimited });
    expect(el.querySelector("[data-testid=api-error-card]")?.getAttribute("data-severity")).toBe("wait");
    expect(
      draw({ info: auth }).querySelector("[data-testid=api-error-card]")?.getAttribute("data-severity"),
    ).toBe("fatal");
  });

  it("offers nothing for safeguards even with a handler wired", () => {
    // A retry would re-send the same prompt into the same classifier.
    expect(actionsOf(draw({ info: safeguards, onAction: vi.fn() }))).toEqual([]);
  });
});
