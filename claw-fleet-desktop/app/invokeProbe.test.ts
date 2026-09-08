// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { installInvokeProbe } from "./invokeProbe";

type Call = [string, unknown];

/** Stand in for Tauri's internals, recording what the probe sends through. */
function harness(handler: (cmd: string) => Promise<unknown>) {
  const calls: Call[] = [];
  const internals = {
    invoke: (cmd: string, args?: unknown) => {
      calls.push([cmd, args]);
      return handler(cmd);
    },
  };
  (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = internals;
  installInvokeProbe();
  const logged = () =>
    calls
      .filter(([cmd]) => cmd === "log_frontend_debug")
      .map(([, args]) => (args as { msg: string }).msg);
  return { internals, calls, logged };
}

beforeEach(() => vi.useFakeTimers());
afterEach(() => {
  vi.useRealTimers();
  delete (window as unknown as { __TAURI_INTERNALS__?: unknown }).__TAURI_INTERNALS__;
});

describe("invoke probe", () => {
  it("stays silent on a fast call", async () => {
    const h = harness(() => Promise.resolve("ok"));
    await h.internals.invoke("list_artifacts");
    expect(h.logged()).toEqual([]);
  });

  /**
   * The load-bearing case: a promise that never settles never reaches any
   * completion branch, which is exactly how `artifact_local_path` hanging left
   * no trace anywhere on 2026-09-08 — two buttons silently absent, zero log
   * lines, and a whole session spent guessing.
   */
  it("reports a call that is still pending", async () => {
    const h = harness(() => new Promise(() => {}));
    void h.internals.invoke("artifact_local_path");
    expect(h.logged()).toEqual([]);
    await vi.advanceTimersByTimeAsync(3_000);
    expect(h.logged()).toHaveLength(1);
    expect(h.logged()[0]).toContain("artifact_local_path");
    expect(h.logged()[0]).toContain("still pending");
  });

  it("reports a slow call once it lands, with its duration", async () => {
    let release: (v: unknown) => void = () => {};
    const h = harness(() => new Promise((r) => (release = r)));
    const pending = h.internals.invoke("plugin:dialog|save");
    await vi.advanceTimersByTimeAsync(1_500);
    release("/tmp/out.zip");
    await pending;
    const lines = h.logged();
    // One "still pending"? No — 1.5s is under the pending threshold, so the
    // only line is the completion one.
    expect(lines).toHaveLength(1);
    expect(lines[0]).toMatch(/plugin:dialog\|save took 1[0-9]{3}ms — ok/);
  });

  it("reports a rejection rather than swallowing it", async () => {
    let fail: (e: unknown) => void = () => {};
    const h = harness(() => new Promise((_, r) => (fail = r)));
    const pending = h.internals.invoke("export_artifact");
    await vi.advanceTimersByTimeAsync(1_200);
    fail(new Error("disk full"));
    await expect(pending).rejects.toThrow("disk full");
    expect(h.logged()[0]).toContain("rejected");
    expect(h.logged()[0]).toContain("disk full");
  });

  /** Timing the log write would recurse forever. */
  it("never times its own log command", async () => {
    const h = harness(() => new Promise(() => {}));
    void h.internals.invoke("log_frontend_debug", { msg: "hello" });
    await vi.advanceTimersByTimeAsync(10_000);
    // The one call is the caller's own; the probe added nothing.
    expect(h.calls).toHaveLength(1);
  });

  it("installs only once", () => {
    const h = harness(() => Promise.resolve(null));
    const wrapped = h.internals.invoke;
    installInvokeProbe();
    expect(h.internals.invoke).toBe(wrapped);
  });

  it("does not crash startup when Tauri exposes invoke as readonly", async () => {
    const raw = vi.fn(() => Promise.resolve("ok"));
    const internals: Record<string, unknown> = {};
    // Tauri 2.11 installs this property with Object.defineProperty({ value }),
    // whose omitted writable/configurable flags both default to false.
    Object.defineProperty(internals, "invoke", { value: raw });
    (window as unknown as { __TAURI_INTERNALS__: unknown }).__TAURI_INTERNALS__ = internals;

    expect(() => installInvokeProbe()).not.toThrow();
    await (internals.invoke as typeof raw)("list_sessions");
    expect(raw).toHaveBeenCalledWith("list_sessions");
  });
});
