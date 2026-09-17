import { describe, expect, it } from "vitest";
import { rowKeyOf } from "./SessionDetailView";
import type { RawMessage } from "../types";

const msg = (uuid?: string): RawMessage =>
  ({ type: "assistant", uuid, message: { role: "assistant", content: [] } }) as unknown as RawMessage;

describe("rowKeyOf", () => {
  // The bug it pins: the transcript is a tail window and "load earlier messages"
  // prepends 200 rows, so every index slides. Keyed by index, the section the
  // reader had expanded (and their expanded thinking blocks) jumped onto a
  // different message.
  it("is the record's own identity, unchanged when the window shifts", () => {
    const m = msg("u-7");
    expect(rowKeyOf(m, 3)).toBe("u-7");
    expect(rowKeyOf(m, 203)).toBe("u-7");
  });

  it("falls back to position only for records with no uuid, and keeps them distinct", () => {
    expect(rowKeyOf(msg(), 3)).toBe("i3");
    expect(rowKeyOf(msg(), 4)).not.toBe(rowKeyOf(msg(), 3));
    expect(rowKeyOf(undefined, 0)).toBe("i0");
  });
});
