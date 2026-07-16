import { describe, it, expect } from "vitest";
import { slicePrecedingMessages } from "./usePrecedingAgentMessages";
import type { RawMessage } from "../types";

function userPrompt(text: string): RawMessage {
  return { type: "user", message: { role: "user", content: text } };
}

function assistantText(text: string): RawMessage {
  return {
    type: "assistant",
    message: { role: "assistant", content: [{ type: "text", text }] },
  };
}

function askCall(id: string, name: string): RawMessage {
  return {
    type: "assistant",
    message: {
      role: "assistant",
      content: [{ type: "tool_use", id, name, input: {} }],
    },
  };
}

function askResult(toolUseId: string, isError: boolean): RawMessage {
  return {
    type: "user",
    message: {
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: toolUseId,
          content: isError ? "InputValidationError: ..." : "answered",
          ...(isError ? { is_error: true } : {}),
        },
      ],
    },
  };
}

function texts(msgs: { text: string }[]): string[] {
  return msgs.map((m) => m.text);
}

describe("slicePrecedingMessages", () => {
  it("includes narration since the last typed prompt", () => {
    const msgs = [
      userPrompt("go investigate"),
      assistantText("I checked the process, it's a zombie."),
      askCall("t1", "mcp__fleet__fleet__ask"),
    ];
    expect(texts(slicePrecedingMessages(msgs))).toEqual([
      "I checked the process, it's a zombie.",
    ]);
  });

  it("stops the span at a SUCCESSFULLY answered ask card", () => {
    const msgs = [
      userPrompt("go"),
      assistantText("before the answered card"),
      askCall("t1", "AskUserQuestion"),
      askResult("t1", false), // user answered → boundary
      assistantText("after the answer — this is the real lead-up"),
      askCall("t2", "mcp__fleet__fleet__ask"),
    ];
    expect(texts(slicePrecedingMessages(msgs))).toEqual([
      "after the answer — this is the real lead-up",
    ]);
  });

  it("does NOT treat a FAILED ask card as a boundary (the screenshot bug)", () => {
    // Mirrors the reported case: narration → AskUserQuestion that errored out
    // (InputValidationError, never answered) → the successful fleet__ask card.
    // The narration before the failed card must survive.
    const msgs = [
      userPrompt("观测一下这个 codex 进程"),
      assistantText("进程还在，rollout 冻结 19 分钟，判定僵死但安全可清理。"),
      askCall("t1", "AskUserQuestion"),
      askResult("t1", true), // FAILED — user never answered, must NOT be a boundary
      assistantText(""), // thinking-only turn between → no text
      askCall("t2", "mcp__fleet__fleet__ask"),
    ];
    expect(texts(slicePrecedingMessages(msgs))).toEqual([
      "进程还在，rollout 冻结 19 分钟，判定僵死但安全可清理。",
    ]);
  });
});
