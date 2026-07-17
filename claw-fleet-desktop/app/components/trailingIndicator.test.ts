import { describe, it, expect } from "vitest";
import { trailingIndicator } from "./trailingIndicator";
import type { RawMessage } from "../types";

const asst = (stop: string | null, uuid = "a"): RawMessage =>
  ({
    type: "assistant",
    uuid,
    message: { role: "assistant", content: [{ type: "text", text: "hi" }], stop_reason: stop },
  }) as unknown as RawMessage;

const userMsg = (
  extra: Partial<RawMessage> = {},
  uuid = "u",
): RawMessage =>
  ({
    type: "user",
    uuid,
    message: { role: "user", content: "画个图" },
    ...extra,
  }) as unknown as RawMessage;

describe("trailingIndicator", () => {
  it("shows waiting when the last turn is an assistant that ended", () => {
    expect(trailingIndicator([userMsg(), asst("end_turn")], null)).toBe("waiting");
  });

  it("shows working when the scanner status is a working status", () => {
    expect(trailingIndicator([asst("end_turn")], "thinking")).toBe("working");
  });

  // The regression: a fresh user prompt after a completed turn is the resume
  // gap — the agent is about to run, so it must NOT read as "waiting for input"
  // even though the previous assistant record still carries end_turn and the
  // scanner status has not flipped yet.
  it("shows working (not waiting) during the resume gap", () => {
    const msgs = [asst("end_turn", "a1"), userMsg({}, "u2")];
    expect(trailingIndicator(msgs, null)).toBe("working");
    expect(trailingIndicator(msgs, "idle")).toBe("working");
  });

  it("still waits when a meta/system user turn trails an ended assistant", () => {
    const msgs = [asst("end_turn", "a1"), userMsg({ isMeta: true }, "u2")];
    expect(trailingIndicator(msgs, null)).toBe("waiting");
  });

  it("does not wait mid-tool-call (assistant stop_reason tool_use)", () => {
    expect(trailingIndicator([userMsg(), asst("tool_use")], null)).toBeNull();
  });

  it("returns null for an empty transcript", () => {
    expect(trailingIndicator([], null)).toBeNull();
  });
});
