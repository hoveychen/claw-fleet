import { describe, it, expect } from "vitest";
import { groupMetaRuns, isMetaRow } from "./metaGrouping";
import type { RawMessage } from "../types";

function meta(): RawMessage {
  return { type: "user", isMeta: true };
}
function user(): RawMessage {
  return { type: "user" };
}
function assistant(): RawMessage {
  return { type: "assistant" };
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
    const units = groupMetaRuns([meta(), meta(), meta()]);
    expect(units).toHaveLength(1);
    expect(units[0]).toMatchObject({ kind: "meta-group", startLocal: 0 });
    expect((units[0] as { msgs: RawMessage[] }).msgs).toHaveLength(3);
  });

  it("leaves a lone meta row as a single (needs >= 2 to group)", () => {
    const units = groupMetaRuns([meta()]);
    expect(units).toEqual([{ kind: "single", startLocal: 0, msg: meta() }]);
  });

  it("does not merge meta rows separated by a normal turn", () => {
    const units = groupMetaRuns([meta(), user(), meta()]);
    expect(units.map((u) => u.kind)).toEqual(["single", "single", "single"]);
  });

  it("preserves startLocal so the render site keeps stable keys", () => {
    // assistant, [meta meta meta], user → single(0), group@1, single(4)
    const units = groupMetaRuns([assistant(), meta(), meta(), meta(), user()]);
    expect(units).toHaveLength(3);
    expect(units[0]).toMatchObject({ kind: "single", startLocal: 0 });
    expect(units[1]).toMatchObject({ kind: "meta-group", startLocal: 1 });
    expect(units[2]).toMatchObject({ kind: "single", startLocal: 4 });
  });

  it("handles a two-row group followed by a trailing lone meta", () => {
    const units = groupMetaRuns([meta(), meta(), assistant(), meta()]);
    expect(units.map((u) => u.kind)).toEqual(["meta-group", "single", "single"]);
    expect((units[0] as { msgs: RawMessage[] }).msgs).toHaveLength(2);
  });
});
