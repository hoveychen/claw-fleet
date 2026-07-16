import { describe, it, expect } from "vitest";
import { groupMetaRuns, isMetaRow } from "./metaGrouping";
import type { RawMessage } from "../types";

const DAY1 = "2026-07-16T10:00:00.000Z";
const DAY1_LATER = "2026-07-16T10:05:00.000Z";
const DAY2 = "2026-07-17T10:00:00.000Z";

function meta(ts = DAY1): RawMessage {
  return { type: "user", isMeta: true, timestamp: ts };
}
function user(ts = DAY1): RawMessage {
  return { type: "user", timestamp: ts };
}
function assistant(ts = DAY1): RawMessage {
  return { type: "assistant", timestamp: ts };
}

describe("isMetaRow", () => {
  it("is true only for isMeta user turns that are not compact summaries", () => {
    expect(isMetaRow(meta())).toBe(true);
    expect(isMetaRow(user())).toBe(false);
    expect(isMetaRow(assistant())).toBe(false);
    expect(isMetaRow({ type: "user", isMeta: true, isCompactSummary: true })).toBe(false);
  });
});

describe("groupMetaRuns", () => {
  it("folds a run of adjacent meta rows into one meta-group", () => {
    const msgs = [meta(DAY1), meta(DAY1), meta(DAY1_LATER)];
    const units = groupMetaRuns(msgs);
    expect(units).toHaveLength(1);
    expect(units[0]).toMatchObject({ kind: "meta-group", startLocal: 0 });
    expect((units[0] as { msgs: RawMessage[] }).msgs).toHaveLength(3);
  });

  it("leaves a lone meta row as a single (needs >= 2 to group)", () => {
    const units = groupMetaRuns([meta()]);
    expect(units).toEqual([{ kind: "single", startLocal: 0, msg: meta() }]);
  });

  it("does not merge meta rows separated by a normal turn", () => {
    const msgs = [meta(DAY1), user(DAY1), meta(DAY1)];
    const units = groupMetaRuns(msgs);
    expect(units.map((u) => u.kind)).toEqual(["single", "single", "single"]);
  });

  it("breaks a run at a day boundary so day separators stay intact", () => {
    const msgs = [meta(DAY1), meta(DAY1), meta(DAY2), meta(DAY2)];
    const units = groupMetaRuns(msgs);
    expect(units).toHaveLength(2);
    expect(units[0]).toMatchObject({ kind: "meta-group", startLocal: 0 });
    expect(units[1]).toMatchObject({ kind: "meta-group", startLocal: 2 });
  });

  it("preserves startLocal so global indices survive grouping", () => {
    // assistant, [meta meta meta], user  → single(0), group@1, single(4)
    const msgs = [assistant(), meta(), meta(), meta(), user()];
    const units = groupMetaRuns(msgs);
    expect(units).toHaveLength(3);
    expect(units[0]).toMatchObject({ kind: "single", startLocal: 0 });
    expect(units[1]).toMatchObject({ kind: "meta-group", startLocal: 1 });
    expect(units[2]).toMatchObject({ kind: "single", startLocal: 4 });
  });

  it("mixes a two-row group and a trailing lone meta correctly", () => {
    const msgs = [meta(DAY1), meta(DAY1), assistant(DAY1), meta(DAY1)];
    const units = groupMetaRuns(msgs);
    expect(units.map((u) => u.kind)).toEqual(["meta-group", "single", "single"]);
    expect((units[0] as { msgs: RawMessage[] }).msgs).toHaveLength(2);
  });
});
