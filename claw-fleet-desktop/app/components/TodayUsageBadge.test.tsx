// @vitest-environment jsdom
import { act } from "react";
import { createRoot, type Root } from "react-dom/client";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import "../i18n";

const invoke = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke }));
vi.mock("@tauri-apps/api/event", () => ({ listen: () => Promise.resolve(() => {}) }));

import { resetSingleFlight } from "../singleFlight";
import { TodayUsageBadge } from "./TodayUsageBadge";

(globalThis as unknown as { IS_REACT_ACT_ENVIRONMENT: boolean }).IS_REACT_ACT_ENVIRONMENT = true;

let container: HTMLDivElement | null = null;
let root: Root | null = null;

beforeEach(() => {
  invoke.mockReset();
  // The pending-case test parks a promise that never settles, so its key would
  // otherwise still be in flight when the next test asks for the same read.
  resetSingleFlight();
});

afterEach(() => {
  if (root) act(() => root!.unmount());
  root = null;
  container?.remove();
  container = null;
});

function render() {
  container = document.createElement("div");
  document.body.appendChild(container);
  root = createRoot(container);
  act(() => root!.render(<TodayUsageBadge />));
  return container;
}

describe("TodayUsageBadge", () => {
  /// A cold launch leaves `today_usage` in flight for as long as the backend
  /// takes to answer (64s on 2026-09-14, before the dsh fold was fixed). While
  /// it is pending the badge knows nothing, and "nothing" must not be drawn as
  /// the perfectly valid figure `$0.00` — that is what made the counter look
  /// broken next to a live spend rate.
  it("shows a pending marker, not $0.00, before the first answer lands", () => {
    invoke.mockReturnValue(new Promise(() => {})); // never resolves
    const el = render();
    expect(el.textContent).toContain("—");
    expect(el.textContent).not.toContain("$0.00");
  });

  it("shows $0.00 once the backend actually reports a zero day", async () => {
    invoke.mockResolvedValue({
      date: "2026-09-14",
      inputTokens: 0,
      outputTokens: 0,
      costUsd: 0,
      agentCostUsd: 0,
      fleetCostUsd: 0,
      sessionCount: 0,
    });
    const el = render();
    await act(async () => {
      await Promise.resolve();
    });
    expect(el.textContent).toContain("$0.00");
  });
});
