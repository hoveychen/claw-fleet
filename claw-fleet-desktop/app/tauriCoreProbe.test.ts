// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from "vitest";

// Through the *aliased* specifier, exactly as every other module reaches it —
// so this test fails if the vite.config alias is ever dropped and the import
// silently resolves to the unwrapped module.
import { invoke } from "@tauri-apps/api/core";

/** Calls the host recorded so a test can assert what the probe logged. */
function stubHost() {
  const calls: Array<{ cmd: string; args: unknown }> = [];
  const pending = new Map<string, (v: unknown) => void>();
  const hostInvoke = (cmd: string, args: unknown) => {
    calls.push({ cmd, args });
    if (cmd === "log_frontend_debug") return Promise.resolve(null);
    return new Promise((resolve) => pending.set(cmd, resolve));
  };
  Object.defineProperty(window, "__TAURI_INTERNALS__", {
    value: { invoke: hostInvoke, transformCallback: (f: unknown) => f },
    configurable: true,
    writable: true,
  });
  return {
    calls,
    settle: (cmd: string, value: unknown = "ok") => pending.get(cmd)?.(value),
    logged: () =>
      calls
        .filter((c) => c.cmd === "log_frontend_debug")
        .map((c) => (c.args as { msg: string }).msg),
  };
}

describe("tauriCoreProbe", () => {
  beforeEach(() => {
    vi.useFakeTimers();
  });

  it("forwards the call and its result untouched", async () => {
    const host = stubHost();
    const p = invoke<string>("get_messages_tail", { tail: 150 });
    expect(host.calls[0]).toEqual({
      cmd: "get_messages_tail",
      args: { tail: 150 },
    });
    host.settle("get_messages_tail", "transcript");
    await expect(p).resolves.toBe("transcript");
  });

  /** The load-bearing half. A promise that never settles never reaches the
   *  completion branch — that is exactly how the >20s stall left no trace. */
  it("reports a call that is still outstanding", async () => {
    const host = stubHost();
    void invoke("get_messages_tail");
    expect(host.logged()).toEqual([]);
    await vi.advanceTimersByTimeAsync(3_000);
    expect(host.logged()).toEqual([
      "[invoke] get_messages_tail still pending after 3000ms",
    ]);
  });

  it("reports a slow call once it lands, with its outcome", async () => {
    const host = stubHost();
    const p = invoke("scan_all_sources");
    await vi.advanceTimersByTimeAsync(5_000);
    host.settle("scan_all_sources");
    await p;
    expect(host.logged().some((m) => /took 5000ms — ok/.test(m))).toBe(true);
  });

  it("reports a rejection too, so a fast failure is not invisible", async () => {
    const host = stubHost();
    Object.defineProperty(window, "__TAURI_INTERNALS__", {
      value: {
        invoke: (cmd: string, args: unknown) => {
          host.calls.push({ cmd, args });
          if (cmd === "log_frontend_debug") return Promise.resolve(null);
          return Promise.reject(new Error("no agent source"));
        },
      },
      configurable: true,
      writable: true,
    });
    await expect(invoke("get_messages_tail")).rejects.toThrow("no agent source");
    // Fast rejection is below the slow threshold, so nothing is logged — the
    // pending timer must still have been cleared rather than firing later.
    await vi.advanceTimersByTimeAsync(10_000);
    expect(host.logged()).toEqual([]);
  });

  it("never times its own log command, or it would recurse", async () => {
    const host = stubHost();
    await invoke("log_frontend_debug", { msg: "hi" });
    await vi.advanceTimersByTimeAsync(10_000);
    // Exactly the one call the test made — no probe line about it.
    expect(host.calls.filter((c) => c.cmd === "log_frontend_debug")).toHaveLength(1);
  });
});
