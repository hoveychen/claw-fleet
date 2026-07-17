import { afterEach, describe, expect, it, vi } from "vitest";

const listeners = new Map<string, (event: { payload: string }) => void>();
const unlisten = vi.fn();

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, handler: (event: { payload: string }) => void) => {
    listeners.set(event, handler);
    return unlisten;
  }),
}));

import { subscribeDecisionHistoryRefresh } from "./decisionHistoryRefresh";

describe("subscribeDecisionHistoryRefresh", () => {
  afterEach(() => {
    listeners.clear();
    unlisten.mockClear();
    vi.useRealTimers();
  });

  it("refreshes after a fleet-ask dismissal has had time to persist history", async () => {
    vi.useFakeTimers();
    const refresh = vi.fn();
    const dispose = subscribeDecisionHistoryRefresh(refresh, 250);

    listeners.get("fleet-ask-dismissed")?.({ payload: "card-1" });
    expect(refresh).not.toHaveBeenCalled();

    await vi.advanceTimersByTimeAsync(250);
    expect(refresh).toHaveBeenCalledTimes(1);

    dispose();
  });

  it("cancels a pending refresh when the session detail unmounts", async () => {
    vi.useFakeTimers();
    const refresh = vi.fn();
    const dispose = subscribeDecisionHistoryRefresh(refresh, 250);

    listeners.get("elicitation-dismissed")?.({ payload: "card-2" });
    dispose();
    await vi.advanceTimersByTimeAsync(250);

    expect(refresh).not.toHaveBeenCalled();
  });
});
