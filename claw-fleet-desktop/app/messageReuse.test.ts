import { describe, expect, it } from "vitest";

import { reconcileMessages } from "./messageReuse";
import type { RawMessage } from "./types";

const msg = (uuid: string, text: string): RawMessage => ({
  type: "assistant",
  uuid,
  timestamp: `2026-08-17T09:0${uuid}:00Z`,
  message: { role: "assistant", content: [{ type: "text", text }] },
});

/** What the IPC boundary hands back: same data, all-new objects. */
const refetched = (msgs: RawMessage[]): RawMessage[] =>
  JSON.parse(JSON.stringify(msgs)) as RawMessage[];

describe("reconcileMessages", () => {
  it("returns the very same array when the poll brought nothing new", () => {
    // The common case by far: the tail poll fires every 1.5s whether or not
    // the agent wrote anything. Handing back a new array re-runs every memo
    // keyed on it and re-renders every row's markdown for no reason at all.
    const prev = [msg("1", "hello"), msg("2", "world")];
    expect(reconcileMessages(prev, refetched(prev))).toBe(prev);
  });

  it("keeps the old objects for settled history when a message is appended", () => {
    const prev = [msg("1", "hello"), msg("2", "world")];
    const next = refetched([...prev, msg("3", "new")]);

    const out = reconcileMessages(prev, next);

    expect(out).toHaveLength(3);
    // Rows above the tail are memoised on the message object; reusing it is
    // what lets them skip re-rendering.
    expect(out[0]).toBe(prev[0]);
    expect(out[1]).toBe(prev[1]);
    expect(out[2]).toBe(next[2]);
  });

  it("takes the fresh copy when the last record was revised", () => {
    const prev = [msg("1", "hello"), msg("2", "partial")];
    const next = refetched([msg("1", "hello"), msg("2", "complete")]);

    const out = reconcileMessages(prev, next);

    expect(out[0]).toBe(prev[0]);
    expect(out[1]).not.toBe(prev[1]);
    expect(out[1]).toEqual(next[1]);
  });

  it("keeps the window sliding forward when the oldest message drops off", () => {
    const prev = [msg("1", "a"), msg("2", "b"), msg("3", "c")];
    const next = refetched([msg("2", "b"), msg("3", "c"), msg("4", "d")]);

    const out = reconcileMessages(prev, next);

    expect(out).toHaveLength(3);
    expect(out[0]).toBe(prev[1]);
    expect(out[1]).toBe(prev[2]);
    expect(out[2]).toEqual(next[2]);
  });

  it("matches on timestamp and message id when there is no uuid", () => {
    // codex / dsh records are normalised and don't always carry a uuid.
    const noUuid = (id: string, text: string): RawMessage => ({
      type: "assistant",
      timestamp: `2026-08-17T09:0${id}:00Z`,
      message: { role: "assistant", id, content: [{ type: "text", text }] },
    });
    const prev = [noUuid("a", "one"), noUuid("b", "two")];
    const next = refetched([...prev, noUuid("c", "three")]);

    const out = reconcileMessages(prev, next);

    expect(out[0]).toBe(prev[0]);
    expect(out[1]).toBe(prev[1]);
  });

  it("never reuses a record with nothing stable to match on", () => {
    const anon = (): RawMessage => ({ type: "progress" });
    const prev = [anon(), anon()];
    const next = refetched(prev);

    const out = reconcileMessages(prev, next);

    expect(out[0]).not.toBe(prev[0]);
    expect(out[1]).not.toBe(prev[1]);
  });

  it("handles the empty edges without inventing content", () => {
    const prev = [msg("1", "hello")];
    expect(reconcileMessages([], prev)).toBe(prev);
    expect(reconcileMessages(prev, [])).toEqual([]);
  });
});
