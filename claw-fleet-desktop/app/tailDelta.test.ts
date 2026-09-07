import { describe, expect, it } from "vitest";

import { appendTailDelta } from "./tailDelta";
import type { RawMessage } from "./types";

const row = (uuid: string | undefined, text: string): RawMessage =>
  ({ type: "assistant", uuid, message: { content: [{ type: "text", text }] } }) as RawMessage;

describe("appendTailDelta", () => {
  it("appends a delta that shares nothing with what is on screen", () => {
    const prev = [row("a", "1"), row("b", "2")];
    const out = appendTailDelta(prev, [row("c", "3")]);
    expect(out.map((m) => m.uuid)).toEqual(["a", "b", "c"]);
  });

  it("hands back the same array when the delta is empty", () => {
    const prev = [row("a", "1")];
    expect(appendTailDelta(prev, [])).toBe(prev);
  });

  it("hands back the same array when every record is already held", () => {
    // Codex re-normalises a trailing window on each poll, so consecutive
    // pushes overlap; re-rendering the transcript for them would be pure waste.
    const prev = [row("a", "1"), row("b", "2")];
    expect(appendTailDelta(prev, [row("a", "1"), row("b", "2")])).toBe(prev);
  });

  it("keeps only the unseen part of an overlapping delta", () => {
    const prev = [row("a", "1"), row("b", "2")];
    const out = appendTailDelta(prev, [row("b", "2"), row("c", "3")]);
    expect(out.map((m) => m.uuid)).toEqual(["a", "b", "c"]);
  });

  it("keeps every record that has no uuid to key on", () => {
    const prev = [row("a", "1")];
    const out = appendTailDelta(prev, [row(undefined, "x"), row(undefined, "y")]);
    expect(out).toHaveLength(3);
  });
});
