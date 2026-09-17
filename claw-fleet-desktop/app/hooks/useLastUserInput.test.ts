import { describe, it, expect } from "vitest";
import { findLastUserInput, formatAnswer, stripPromptEnvelope } from "./useLastUserInput";
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
    message: { role: "assistant", content: [{ type: "tool_use", id, name, input: {} }] },
  } as RawMessage;
}

function askResult(id: string, content: string, isError = false): RawMessage {
  return {
    type: "user",
    message: {
      role: "user",
      content: [{ type: "tool_result", tool_use_id: id, content, is_error: isError }],
    },
  } as RawMessage;
}

function bashResult(id: string, content: string): RawMessage {
  return {
    type: "user",
    message: {
      role: "user",
      content: [{ type: "tool_result", tool_use_id: id, content }],
    },
  } as RawMessage;
}

describe("findLastUserInput", () => {
  it("returns the typed prompt when the user last typed", () => {
    const msgs = [
      userPrompt("修一下这个 bug"),
      assistantText("我先看看代码。"),
    ];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "修一下这个 bug" });
  });

  it("returns the previous card's answer, not the question it answered", () => {
    const msgs = [
      userPrompt("开始吧"),
      askCall("t1", "mcp__fleet__fleet__ask"),
      askResult("t1", JSON.stringify({ answers: { "很长很长的问题正文…": "先做 A 方案" } })),
      assistantText("好的，我按 A 方案来。"),
    ];
    expect(findLastUserInput(msgs)).toEqual({ kind: "answer", text: "先做 A 方案" });
  });

  it("ignores plain tool results — Bash output is not the user speaking", () => {
    const msgs = [
      userPrompt("跑一下测试"),
      askCall("t1", "Bash"),
      bashResult("t1", "ok"),
    ];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "跑一下测试" });
  });

  it("skips a failed ask call and falls back to the earlier real input", () => {
    const msgs = [
      userPrompt("继续"),
      askCall("t1", "AskUserQuestion"),
      askResult("t1", "InputValidationError", true),
      assistantText("卡发失败了，重发一张。"),
    ];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "继续" });
  });

  it("returns null when the session has no earlier user input", () => {
    expect(findLastUserInput([assistantText("开场白")])).toBeNull();
  });

  it("strips the system-reminder envelope from a typed prompt", () => {
    const msgs = [
      userPrompt("<system-reminder>internal junk</system-reminder>\n真正说的话"),
    ];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "真正说的话" });
  });

  it("keeps looking back when a message is envelope-only", () => {
    const msgs = [
      userPrompt("原始需求"),
      assistantText("干活中"),
      userPrompt("<system-reminder>hook context</system-reminder>"),
    ];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "原始需求" });
  });
});

describe("formatAnswer", () => {
  it("joins multiple answer values", () => {
    const raw = JSON.stringify({ answers: { q1: "选 A", note: "顺便清理一下" } });
    expect(formatAnswer(raw)).toBe("选 A\n顺便清理一下");
  });

  it("passes through non-JSON payloads like TASK FINISHED", () => {
    expect(formatAnswer("TASK FINISHED")).toBe("TASK FINISHED");
  });
});

describe("stripPromptEnvelope", () => {
  it("drops slash-command envelopes", () => {
    expect(
      stripPromptEnvelope("<command-name>/loop</command-name>\n手写内容"),
    ).toBe("手写内容");
  });
});
