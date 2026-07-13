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

  it("第一次返回只弹提示，不离开", () => {
    const { guard, armedChange, leave } = mk();
    expect(guard.handleRootBack()).toBe("hold");
    expect(armedChange).toHaveBeenLastCalledWith(true);
    expect(leave).not.toHaveBeenCalled();
  });

  it("窗口内再按一次才放行，并先摘掉 beforeunload", () => {
    const { guard, armedChange, leave } = mk();
    guard.handleRootBack();
    vi.advanceTimersByTime(EXIT_WINDOW_MS - 1);

    expect(guard.handleRootBack()).toBe("leave");
    expect(leave).toHaveBeenCalledTimes(1);
    expect(armedChange).toHaveBeenLastCalledWith(false); // toast 收起
  });

  it("超过窗口后重新武装：又只是提示，不会漏放行", () => {
    const { guard, armedChange, leave } = mk();
    guard.handleRootBack();
    vi.advanceTimersByTime(EXIT_WINDOW_MS);
    expect(armedChange).toHaveBeenLastCalledWith(false); // 自动收起

    expect(guard.handleRootBack()).toBe("hold");
    expect(leave).not.toHaveBeenCalled();
  });
});
