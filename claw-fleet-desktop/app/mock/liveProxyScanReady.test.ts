// @vitest-environment jsdom
// The browser build stands in for the desktop's one-shot `scan-ready` event.
// Without it a machine with no sessions kept the home task list on its
// skeleton forever: the store only infers readiness from a non-empty list.
import { afterEach, describe, expect, it, vi } from "vitest";

const emitted: Array<[string, unknown]> = [];
vi.mock("@tauri-apps/api/event", () => ({
  emit: async (name: string, payload: unknown) => {
    emitted.push([name, payload]);
  },
}));

const realFetch = globalThis.fetch;
afterEach(() => {
  globalThis.fetch = realFetch;
});

describe("scan-ready in the browser build", () => {
  it("is announced after the first sessions snapshot, even an empty one", async () => {
    globalThis.fetch = (async () =>
      new Response(JSON.stringify({ kind: "full", seq: 1, sessions: [] }), {
        status: 200,
        headers: { "Content-Type": "application/json" },
      })) as typeof fetch;
    const { liveInvoke } = await import("./liveProxy");

    const res = await liveInvoke("sessions_snapshot", {});
    expect(res.value).toEqual({ kind: "full", seq: 1, sessions: [] });
    // Deferred a tick, so the frame is applied before readiness lands.
    expect(emitted.filter(([n]) => n === "scan-ready")).toHaveLength(0);
    await new Promise((r) => setTimeout(r, 0));
    expect(emitted.filter(([n]) => n === "scan-ready")).toEqual([["scan-ready", true]]);

    // Once is enough: later snapshots do not re-announce it.
    await liveInvoke("sessions_snapshot", {});
    await new Promise((r) => setTimeout(r, 0));
    expect(emitted.filter(([n]) => n === "scan-ready")).toHaveLength(1);
  });
});
