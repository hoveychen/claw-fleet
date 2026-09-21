// The phone's side-question surface is three relay method names and one
// idempotency key. The drift guard (claw-fleet-core/tests/mobile_relay_drift_guard.rs)
// proves the names exist on the host; this pins what travels with them — the
// ask's key in particular, since a retried ask without it forks (and bills)
// twice.

import { describe, expect, it } from "vitest";

import type { ExplainRequest } from "./generated/types";
import { askExplanation, getExplanation, listExplanations, refusedExplanation } from "./sessionExplain";
import type { FleetTransport } from "./transport";

function fakeTransport() {
  const calls: Array<{ method: string; params: Record<string, unknown> | undefined }> = [];
  const client = {
    request: <T,>(method: string, params?: Record<string, unknown>) => {
      calls.push({ method, params });
      return Promise.resolve({ method } as unknown as T);
    },
  } as unknown as FleetTransport;
  return { client, calls };
}

const REQ: ExplainRequest = {
  sessionId: "sess-1",
  sessionPath: "/tmp/sess-1.jsonl",
  workspacePath: "/tmp/ws",
  quote: "fork 命中缓存",
  preset: "explain",
  anchor: { msgUuid: "am2", msgIdx: 3 },
  thread: [],
};

describe("sessionExplain transport wrappers", () => {
  it("ask sends the request fields flat with an idempotency key alongside", async () => {
    const { client, calls } = fakeTransport();
    await askExplanation(client, REQ, "key-1");
    expect(calls).toHaveLength(1);
    expect(calls[0].method).toBe("session_explain_ask");
    expect(calls[0].params).toEqual({ ...REQ, idempotencyKey: "key-1" });
  });

  it("ask mints a key when none is given, never an empty one", async () => {
    const { client, calls } = fakeTransport();
    await askExplanation(client, REQ);
    const key = calls[0].params?.idempotencyKey;
    expect(typeof key).toBe("string");
    expect((key as string).length).toBeGreaterThan(8);
  });

  it("get and list address the record by session and id", async () => {
    const { client, calls } = fakeTransport();
    await getExplanation(client, "sess-1", "rec-9");
    await listExplanations(client, "sess-1");
    expect(calls.map((c) => c.method)).toEqual(["session_explain", "session_explain_list"]);
    expect(calls[0].params).toEqual({ sessionId: "sess-1", id: "rec-9" });
    expect(calls[1].params).toEqual({ sessionId: "sess-1" });
  });
});

describe("refusedExplanation", () => {
  it("is a failed, local-only record carrying the host's message where the answer would be", () => {
    const rec = refusedExplanation(REQ, new Error("no source owns the session"));
    expect(rec.status).toBe("error");
    expect(rec.error).toBe("no source owns the session");
    expect(rec.id.startsWith("local-")).toBe(true);
    expect(rec.sessionId).toBe(REQ.sessionId);
    expect(rec.quote).toBe(REQ.quote);
    expect(rec.anchor).toEqual(REQ.anchor);
    expect(rec.text).toBe("");
  });

  it("stringifies a non-Error rejection instead of dropping it", () => {
    expect(refusedExplanation(REQ, "offline").error).toBe("offline");
  });
});
