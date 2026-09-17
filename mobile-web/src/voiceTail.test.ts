import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("@capacitor/core", () => ({
  Capacitor: { isNativePlatform: () => false },
}));

const { createTailGuard, TAIL_GRACE_MS } = await import("./voiceTail");

beforeEach(() => {
  vi.useFakeTimers();
});

afterEach(() => {
  vi.useRealTimers();
});

describe("createTailGuard", () => {
  // This is the path where "you finish speaking in decision card's 'Other', click stop,
  // and the text disappears entirely": the engine doesn't emit a final transcript
  // after finish (Harmony's end event arrives first and tears down the callback hook),
  // so that last segment only in the live display has no exit.
  it("stops after engine failed to emit transcript → commit the final live display at grace period", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("合一下 worktree");
    g.stop();
    expect(commit).not.toHaveBeenCalled(); // give engine time to emit transcript first
    vi.advanceTimersByTime(TAIL_GRACE_MS);
    expect(commit).toHaveBeenCalledExactlyOnceWith("合一下 worktree");
  });

  it("engine emitted transcript → don't commit, avoid duplicating the same sentence", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("合一下");
    g.stop();
    g.final(); // transcript goes through useVoiceInput's own path, this just marks "it arrived"
    vi.advanceTimersByTime(TAIL_GRACE_MS * 3);
    expect(commit).not.toHaveBeenCalled();
  });

  it("cancel = discard → don't commit that display segment", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("不要这句");
    g.cancel();
    vi.advanceTimersByTime(TAIL_GRACE_MS * 3);
    expect(commit).not.toHaveBeenCalled();
  });

  it("stop when hearing nothing → don't commit empty string", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.stop();
    vi.advanceTimersByTime(TAIL_GRACE_MS * 3);
    expect(commit).not.toHaveBeenCalled();
  });

  it("new display arrives after transcript, stop commits the new one", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("第一句");
    g.final(); // engine itself breaks sentences, first one is already in input box
    g.partial("第二句");
    g.stop();
    vi.advanceTimersByTime(TAIL_GRACE_MS);
    // committing the first sentence would duplicate what's already in the input box.
    expect(commit).toHaveBeenCalledExactlyOnceWith("第二句");
  });

  it("component unmounted → don't commit, input box is gone", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("说到一半页面被关了");
    g.stop();
    g.dispose();
    vi.advanceTimersByTime(TAIL_GRACE_MS * 3);
    expect(commit).not.toHaveBeenCalled();
  });

  // Engine still emits live display after stop (web-speech's stop is async cleanup) —
  // commit what the user last saw on screen, not the old value at the moment they hit stop.
  it("display arriving after stop updates pending commit content", () => {
    const commit = vi.fn();
    const g = createTailGuard(commit);
    g.partial("合一下 work");
    g.stop();
    g.partial("合一下 worktree");
    vi.advanceTimersByTime(TAIL_GRACE_MS);
    expect(commit).toHaveBeenCalledExactlyOnceWith("合一下 worktree");
  });
});

// Commit lands 900ms later. If the input box clears it and brings it back during that time,
// the user still sees "text disappeared" — just for less time. So the guard must tell
// the caller "I'm still waiting" so the UI keeps that text on screen.
describe("createTailGuard waiting state", () => {
  it("with pending transcript, stop returns true; after commit returns true to onSettle", () => {
    const onSettle = vi.fn();
    const g = createTailGuard(vi.fn(), { onSettle });
    g.partial("最后一句");
    expect(g.stop()).toBe(true);
    expect(onSettle).not.toHaveBeenCalled();
    vi.advanceTimersByTime(TAIL_GRACE_MS);
    expect(onSettle).toHaveBeenCalledTimes(1);
  });

  it("no text: stop doesn't enter waiting state — UI shouldn't hold an extra frame", () => {
    const onSettle = vi.fn();
    const g = createTailGuard(vi.fn(), { onSettle });
    expect(g.stop()).toBe(false);
  });

  it("transcript arrives early → end waiting immediately, don't wait until grace period", () => {
    const onSettle = vi.fn();
    const g = createTailGuard(vi.fn(), { onSettle });
    g.partial("最后一句");
    g.stop();
    g.final();
    expect(onSettle).toHaveBeenCalledTimes(1);
  });

  it("cancel also ends waiting", () => {
    const onSettle = vi.fn();
    const g = createTailGuard(vi.fn(), { onSettle });
    g.partial("最后一句");
    g.stop();
    g.cancel();
    expect(onSettle).toHaveBeenCalledTimes(1);
  });

  it("not waiting: transcript/cancel don't falsely report settled once", () => {
    const onSettle = vi.fn();
    const g = createTailGuard(vi.fn(), { onSettle });
    g.partial("说着呢");
    g.final();
    g.cancel();
    expect(onSettle).not.toHaveBeenCalled();
  });
});
