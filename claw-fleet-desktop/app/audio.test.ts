// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from "vitest";

import { ensureAudioContext, __resetAudioContextForTests } from "./audio";

/**
 * Stand-in for the browser's AudioContext.
 *
 * `hang` is the failure this whole guard exists for: on WKWebView `resume()`
 * can never settle when called outside a user gesture. `hangFirstOnly` models
 * the documented assumption that the replacement — built inside the current
 * user-gesture call stack — does start.
 */
class FakeAudioContext {
  static created: FakeAudioContext[] = [];
  static hang = false;
  static hangFirstOnly = true;

  state: AudioContextState = "suspended";
  closed = false;
  private readonly hangs: boolean;

  constructor() {
    const isFirst = FakeAudioContext.created.length === 0;
    this.hangs = FakeAudioContext.hang && (isFirst || !FakeAudioContext.hangFirstOnly);
    FakeAudioContext.created.push(this);
  }

  resume(): Promise<void> {
    if (this.hangs) return new Promise<void>(() => {});
    this.state = "running";
    return Promise.resolve();
  }

  close(): Promise<void> {
    this.closed = true;
    this.state = "closed";
    return Promise.resolve();
  }
}

const install = (opts: { hang?: boolean; hangFirstOnly?: boolean } = {}) => {
  FakeAudioContext.created = [];
  FakeAudioContext.hang = !!opts.hang;
  FakeAudioContext.hangFirstOnly = opts.hangFirstOnly ?? true;
  vi.stubGlobal("AudioContext", FakeAudioContext);
  __resetAudioContextForTests();
};

afterEach(() => {
  vi.unstubAllGlobals();
  __resetAudioContextForTests();
});

describe("ensureAudioContext", () => {
  it("still returns when the replacement's resume hangs too", async () => {
    // The guard was self-defeating: having bounded the *first* resume against
    // a hang, it then awaited the replacement's resume unbounded — the same
    // call, the same hang. A chime would never resolve and its caller would
    // wait forever. Without the bound this test does not fail an assertion, it
    // times out.
    install({ hang: true, hangFirstOnly: false });

    const ctx = (await ensureAudioContext()) as unknown as FakeAudioContext;

    expect(FakeAudioContext.created.length).toBeGreaterThan(1);
    expect(ctx.state).toBe("suspended");
  }, 3000);

  it("replaces a context whose resume never starts it", async () => {
    install({ hang: true });

    const ctx = (await ensureAudioContext()) as unknown as FakeAudioContext;

    expect(FakeAudioContext.created[0].closed).toBe(true);
    expect(ctx).not.toBe(FakeAudioContext.created[0]);
    expect(ctx.state).toBe("running");
  });

  it("leaves an already running context alone", async () => {
    install();
    const first = await ensureAudioContext();
    const second = await ensureAudioContext();

    expect(second).toBe(first);
    expect(FakeAudioContext.created).toHaveLength(1);
  });
});
