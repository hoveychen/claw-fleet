import { describe, expect, it } from "vitest";
import type { RawMessage } from "../types";
import { settlePending } from "./SessionDetailView";

describe("settlePending", () => {
  const user = (text: string, extra: Partial<RawMessage> = {}): RawMessage => ({
    type: "user",
    message: { role: "user", content: [{ type: "text", text }] },
    ...extra,
  });

  it("keeps an unread injected message on screen", () => {
    const rows = [user("先跑测试"), user("这么久的么？", { fleetPending: true })];
    expect(settlePending(rows)).toEqual(rows);
  });

  it("marks a still-unread message stale once the session is gone", () => {
    const [row] = settlePending([user("x", { fleetPending: true })], false);
    expect(row.fleetPendingStale).toBe(true);
  });

  it("drops the pending row once the absorbed copy follows it", () => {
    const absorbed = user("这么久的么？", { fleetMidTurn: true });
    expect(settlePending([user("这么久的么？", { fleetPending: true }), absorbed])).toEqual([absorbed]);
  });
});
