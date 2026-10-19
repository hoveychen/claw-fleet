// @vitest-environment jsdom
//
// Strings are the English ones: `../i18n` initialises to `en` under vitest.
//
// `watchProgressView` is the rule both apps share; the render half checks the
// chip actually draws what the rule decided (ring, line, stalled callout).
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import "../i18n";
import { WatchStatusRow } from "./WatchStatusRow";
import { MOCK_SESSIONS } from "../mock/data";
import type { SessionInfo, WatchSummary } from "../types";
import { WATCH_STALL_MS, watchProgressView, watchRowToken } from "../../../shared-ts/watchProgress";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const MIN = 60_000;

function watch(over: Partial<WatchSummary>): WatchSummary {
  return {
    id: "w1",
    note: "CI run",
    created: Date.now() - 30 * MIN,
    pollSecs: 30,
    deadlineAt: Date.now() + 60 * MIN,
    pollCount: 60,
    structuralFailStreak: 0,
    ...over,
  } as WatchSummary;
}

describe("watchProgressView", () => {
  const now = 1_000 * MIN;

  it("prefers reported progress and flags it stalled only past the threshold", () => {
    const moving = watchProgressView(
      { created: 0, progress: "3/8", progressFraction: 0.375, progressChangedAt: now - MIN },
      now,
    );
    expect(moving).toEqual({ kind: "reported", text: "3/8", fraction: 0.375, stalledMs: null });
    const stuck = watchProgressView(
      { created: 0, progress: "3/8", progressFraction: 0.375, progressChangedAt: now - WATCH_STALL_MS },
      now,
    );
    expect(stuck.kind === "reported" && stuck.stalledMs).toBe(WATCH_STALL_MS);
  });

  it("falls back to elapsed against expect-by, clamped and marked overdue", () => {
    const half = watchProgressView({ created: now - 10 * MIN, expectBy: now + 10 * MIN }, now);
    expect(half).toMatchObject({ kind: "time", fraction: 0.5, overdueMs: null });
    const late = watchProgressView({ created: now - 30 * MIN, expectBy: now - 10 * MIN }, now);
    expect(late).toMatchObject({ kind: "time", fraction: 1, overdueMs: 10 * MIN });
  });

  it("has nothing to say without either source", () => {
    expect(watchProgressView({ created: 0 }, now)).toEqual({ kind: "none" });
  });
});

// The task-row token both apps draw.
describe("watchRowToken", () => {
  const now = 1_000 * MIN;
  const base = { created: now - 30 * MIN, pollCount: 60 };

  it("counts finished targets on a session watch, amber once one failed", () => {
    expect(watchRowToken({ ...base, targets: [{ phase: "done" }, { phase: "running" }] }, now)).toEqual({
      ring: null,
      text: "1/2",
      alarming: false,
    });
    expect(watchRowToken({ ...base, targets: [{ phase: "failed" }] }, now).alarming).toBe(true);
  });

  it("prefers a percentage, then the reported line, then time, then polls", () => {
    const pct = watchRowToken({ ...base, progress: "3/8", progressFraction: 0.375, progressChangedAt: now }, now);
    expect(pct).toMatchObject({ text: "38%", ring: { fraction: 0.375 } });
    expect(watchRowToken({ ...base, progress: "building", progressChangedAt: now }, now)).toMatchObject({
      text: "building",
      ring: null,
    });
    expect(watchRowToken({ ...base, expectBy: now + 30 * MIN }, now).text).toBe("30m/1h");
    expect(watchRowToken(base, now)).toEqual({ ring: null, text: "30m·60×", alarming: false });
  });

  it("is amber when the until command cannot run", () => {
    expect(watchRowToken({ ...base, structuralFailStreak: 3 }, now).alarming).toBe(true);
  });
});

describe("WatchStatusRow progress", () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement("div");
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(() => {
    act(() => root.unmount());
    container.remove();
  });

  function render(w: WatchSummary) {
    const session = { ...MOCK_SESSIONS[0], watches: [w] } as SessionInfo;
    act(() => root.render(<WatchStatusRow session={session} />));
  }

  it("draws a ring and the reported line, and calls out a stalled one", () => {
    render(
      watch({
        progress: "3/8 steps",
        progressFraction: 0.375,
        progressChangedAt: Date.now() - 12 * MIN,
      }),
    );
    const bar = container.querySelector('[role="progressbar"]');
    expect(bar?.getAttribute("aria-valuenow")).toBe("38");
    expect(container.textContent).toContain("3/8 steps");
    expect(container.textContent).toContain("unchanged 12m");
  });

  it("draws elapsed against the expected wait when only expect-by is known", () => {
    render(watch({ expectBy: Date.now() + 30 * MIN }));
    expect(container.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow")).toBe("50");
    expect(container.textContent).toMatch(/30m\d\ds of ~1h00m/);
  });

  it("keeps the poll-count chip for a watch with no progress source", () => {
    render(watch({}));
    expect(container.querySelector('[role="progressbar"]')).toBeNull();
    expect(container.textContent).toContain("60×");
  });
});
