// @vitest-environment jsdom
//
// The task row's watch token: the progress the old session row drew, which the
// task list lost when it replaced that row.
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, describe, expect, it } from "vitest";

import "../i18n";
import type { SessionInfo, WatchSummary } from "../types";
import type { HomeRow } from "./homeRows";
import { RowWatch } from "./TaskList";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

const MIN = 60_000;
const NOW = 1_000 * MIN;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function watch(over: Partial<WatchSummary>): WatchSummary {
  return {
    id: "w1",
    note: "CI run",
    created: NOW - 30 * MIN,
    pollSecs: 30,
    deadlineAt: NOW + 60 * MIN,
    pollCount: 60,
    structuralFailStreak: 0,
    ...over,
  } as WatchSummary;
}

function render(watches: WatchSummary[]) {
  const tip = { id: "main", watches } as unknown as SessionInfo;
  const row = { kind: "task", hops: [tip], tip, subtasks: [] } as unknown as HomeRow;
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<RowWatch row={row} now={NOW} />));
  return container;
}

describe("RowWatch", () => {
  it("draws nothing without a watch", () => {
    expect(render([]).textContent).toBe("");
  });

  it("shows a ring and the percentage for fractional progress", () => {
    const el = render([watch({ progress: "3/8", progressFraction: 0.375, progressChangedAt: NOW - MIN })]);
    expect(el.querySelector('[role="progressbar"]')?.getAttribute("aria-valuenow")).toBe("38");
    expect(el.textContent).toBe("38%");
  });

  it("shows a free-text progress line as is", () => {
    const el = render([watch({ progress: "job running for 4m", progressChangedAt: NOW - MIN })]);
    expect(el.querySelector('[role="progressbar"]')).toBeNull();
    expect(el.textContent).toBe("job running for 4m");
  });

  it("falls back to elapsed and polls, and turns amber past expect-by", () => {
    expect(render([watch({})]).textContent).toBe("30m·60×");
    act(() => root!.unmount());
    root = null;
    const late = render([watch({ expectBy: NOW - 10 * MIN })]);
    expect(late.textContent).toBe("30m/20m");
    expect(late.querySelector("[data-testid='task-row-watch']")?.className).toMatch(/watch_alarming/);
  });
});
