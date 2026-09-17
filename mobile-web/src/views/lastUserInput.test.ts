import { describe, it, expect } from "vitest";
import { findLastUserInput, formatAnswer, stripPromptEnvelope } from "./DecisionsView";
import type { RawMessage } from "../types";

function userPrompt(text: string): RawMessage {
  return { type: "user", message: { role: "user", content: text } } as RawMessage;
}

function assistantText(text: string): RawMessage {
  return {
    type: "assistant",
    message: { role: "assistant", content: [{ type: "text", text }] },
  } as RawMessage;
}

function askCall(id: string, name: string): RawMessage {
  return {
    type: "assistant",
    message: { role: "assistant", content: [{ type: "tool_use", id, name }] },
  } as RawMessage;
}

function toolResult(toolUseId: string, content: string, isError = false): RawMessage {
  return {
    type: "user",
    message: {
      role: "user",
      content: [
        {
          type: "tool_result",
          tool_use_id: toolUseId,
          content,
          ...(isError ? { is_error: true } : {}),
        },
      ],
    },
  } as RawMessage;
}

describe("findLastUserInput (mobile-web)", () => {
  it("returns the typed prompt when the user last typed", () => {
    const msgs = [userPrompt("go investigate"), assistantText("on it")];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "go investigate" });
  });

  it("returns the previous card's answer, not the question it answered", () => {
    const msgs = [
      userPrompt("start"),
      askCall("t1", "mcp__fleet__fleet__ask"),
      toolResult("t1", JSON.stringify({ answers: { "a very long question body…": "plan A" } })),
      assistantText("going with A"),
    ];
    expect(findLastUserInput(msgs)).toEqual({ kind: "answer", text: "plan A" });
  });

  it("ignores plain tool results — Bash output is not the user speaking", () => {
    const msgs = [userPrompt("run the tests"), askCall("t1", "Bash"), toolResult("t1", "ok")];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "run the tests" });
  });

  it("skips a failed ask call and falls back to the earlier real input", () => {
    const msgs = [
      userPrompt("continue"),
      askCall("t1", "AskUserQuestion"),
      toolResult("t1", "InputValidationError: ...", true),
      assistantText("card failed, retrying"),
    ];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "continue" });
  });

  it("returns null when the session has no earlier user input", () => {
    expect(findLastUserInput([assistantText("opening line")])).toBeNull();
  });

  it("strips the system-reminder envelope and keeps looking when nothing is left", () => {
    const msgs = [
      userPrompt("the real request"),
      assistantText("working"),
      userPrompt("<system-reminder>hook context</system-reminder>"),
    ];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "the real request" });
  });
});

describe("formatAnswer (mobile-web)", () => {
  it("joins multiple answer values", () => {
    expect(formatAnswer(JSON.stringify({ answers: { q1: "pick A", note: "also tidy up" } }))).toBe(
      "pick A\nalso tidy up",
    );
  });

  it("passes through non-JSON payloads like TASK FINISHED", () => {
    expect(formatAnswer("TASK FINISHED")).toBe("TASK FINISHED");
  });
});

describe("stripPromptEnvelope (mobile-web)", () => {
  it("drops slash-command envelopes", () => {
    expect(stripPromptEnvelope("<command-name>/loop</command-name>\nhand-written")).toBe(
      "hand-written",
    );
  });
});
