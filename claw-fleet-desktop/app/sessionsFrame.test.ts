import { describe, expect, it } from "vitest";

import { applySessionsFrame, type SessionsState } from "../../shared-ts/sessionsFrame";

type Row = { id: string; v?: number };

const state = (seq: number | null, ...rows: Row[]): SessionsState<Row> => ({ seq, sessions: rows });

describe("applySessionsFrame", () => {
  it("a full frame replaces the list and adopts its seq", () => {
    const next = applySessionsFrame(state(null, { id: "x" }), {
      kind: "full",
      seq: 4,
      sessions: [{ id: "a" }, { id: "b" }],
    });
    expect(next).toEqual(state(4, { id: "a" }, { id: "b" }));
  });

  it("a delta upserts, removes and appends new rows, keeping unchanged rows by identity", () => {
    const a = { id: "a", v: 1 };
    const b = { id: "b", v: 1 };
    const c = { id: "c", v: 1 };
    const next = applySessionsFrame(state(1, a, b, c), {
      kind: "delta",
      seq: 2,
      baseSeq: 1,
      upsert: [{ id: "b", v: 2 }, { id: "d" }],
      remove: ["c"],
    });
    expect(next).toEqual(state(2, a, { id: "b", v: 2 }, { id: "d" }));
    expect(next!.sessions[0]).toBe(a);
  });

  it("a delta's order, when present, is the new order", () => {
    const next = applySessionsFrame(state(1, { id: "a" }, { id: "b" }), {
      kind: "delta",
      seq: 2,
      baseSeq: 1,
      upsert: [],
      remove: [],
      order: ["b", "a"],
    });
    expect(next!.sessions.map((s) => s.id)).toEqual(["b", "a"]);
  });

  it("a delta that does not follow the held seq asks for a resync", () => {
    const delta = { kind: "delta" as const, seq: 5, baseSeq: 4, upsert: [], remove: [] };
    expect(applySessionsFrame(state(3, { id: "a" }), delta)).toBeNull();
    expect(applySessionsFrame(state(null, { id: "a" }), delta)).toBeNull();
  });

  it("frames at or behind the held seq are ignored", () => {
    const held = state(5, { id: "a" });
    expect(
      applySessionsFrame(held, { kind: "delta", seq: 5, baseSeq: 4, upsert: [{ id: "z" }], remove: [] }),
    ).toBe(held);
    expect(applySessionsFrame(held, { kind: "full", seq: 3, sessions: [] })).toBe(held);
  });

  it("a full frame at the held seq is a resync and is taken", () => {
    const next = applySessionsFrame(state(5, { id: "a" }), { kind: "full", seq: 5, sessions: [{ id: "b" }] });
    expect(next).toEqual(state(5, { id: "b" }));
  });
});
