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

  // The backstop. A turn that died without ever writing a terminal record —
  // codex killed mid-flight, a crashed harness — leaves a user prompt at the
  // tail forever. The resume-gap rule above then pins 「处理中…」 on a session
  // where nothing is running, with no timeout to clear it. Once the prompt is
  // minutes old and the scanner does not claim the agent is working, it is not
  // a resume gap any more.
  describe("stale trailing prompt", () => {
    const NOW = 1_700_000_000_000;
    const at = (msAgo: number, uuid = "u2") =>
      userMsg({ timestamp: new Date(NOW - msAgo).toISOString() }, uuid);

    it("stops working after the prompt goes stale with no live status", () => {
      const msgs = [asst("end_turn", "a1"), at(10 * 60_000)];
      expect(trailingIndicator(msgs, null, NOW)).toBe("waiting");
      expect(trailingIndicator(msgs, "idle", NOW)).toBe("waiting");
    });

    it("keeps working while the scanner says the agent is live", () => {
      const msgs = [asst("end_turn", "a1"), at(10 * 60_000)];
      expect(trailingIndicator(msgs, "thinking", NOW)).toBe("working");
      expect(trailingIndicator(msgs, "executing", NOW)).toBe("working");
    });

    it("keeps working during a genuinely fresh resume gap", () => {
      const msgs = [asst("end_turn", "a1"), at(3_000)];
      expect(trailingIndicator(msgs, null, NOW)).toBe("working");
    });

    it("keeps working when the prompt carries no timestamp", () => {
      const msgs = [asst("end_turn", "a1"), userMsg({}, "u2")];
      expect(trailingIndicator(msgs, null, NOW)).toBe("working");
    });
  });
});
