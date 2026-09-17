import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { EXIT_WINDOW_MS, ExitGuard } from "./exitGuard";

describe("ExitGuard", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function mk() {
    const armedChange = vi.fn<(armed: boolean) => void>();
    const leave = vi.fn();
    return { guard: new ExitGuard(armedChange, leave), armedChange, leave };
  }

  it("first back only shows prompt, doesn't leave", () => {
    const { guard, armedChange, leave } = mk();
    expect(guard.handleRootBack()).toBe("hold");
    expect(armedChange).toHaveBeenLastCalledWith(true);
    expect(leave).not.toHaveBeenCalled();
  });

  it("within window, second back allows exit and removes beforeunload first", () => {
    const { guard, armedChange, leave } = mk();
    guard.handleRootBack();
    vi.advanceTimersByTime(EXIT_WINDOW_MS - 1);

    expect(guard.handleRootBack()).toBe("leave");
    expect(leave).toHaveBeenCalledTimes(1);
    expect(armedChange).toHaveBeenLastCalledWith(false); // toast dismissed
  });

  it("after window expires, re-armed: again just prompt, won't skip exit", () => {
    const { guard, armedChange, leave } = mk();
    guard.handleRootBack();
    vi.advanceTimersByTime(EXIT_WINDOW_MS);
    expect(armedChange).toHaveBeenLastCalledWith(false); // auto-dismissed

    expect(guard.handleRootBack()).toBe("hold");
    expect(leave).not.toHaveBeenCalled();
  });
});
