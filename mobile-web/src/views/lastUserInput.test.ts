import { describe, it, expect } from "vitest";
import { answerLabel, findLastUserInput, formatAnswer, stripPromptEnvelope } from "./DecisionsView";
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

  it("returns the previous card's answer with a clipped question label", () => {
    const msgs = [
      userPrompt("start"),
      askCall("t1", "mcp__fleet__fleet__ask"),
      toolResult(
        "t1",
        JSON.stringify({ answers: { "Pick a route.\n---\na very long question body…": "plan A" } }),
      ),
      assistantText("going with A"),
    ];
    expect(findLastUserInput(msgs)).toEqual({
      kind: "answer",
      text: "plan A",
      answers: [{ label: "Pick a route.", value: "plan A" }],
    });
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

  it("skips the isMeta companion row Claude Code writes next to an image read", () => {
    const imageCompanion = {
      type: "user",
      isMeta: true,
      message: {
        role: "user",
        content:
          "[Image: original 2560x1640, displayed at 2000x1281. Multiply coordinates by 1.28 to map to original image.]",
      },
    } as RawMessage;
    const msgs = [userPrompt("看下这张截图"), assistantText("reading it"), imageCompanion];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "看下这张截图" });
  });

  it("returns null when the session has no earlier user input", () => {
    expect(findLastUserInput([assistantText("opening line")])).toBeNull();
  });

  it("drops the composer's trailing Context files block", () => {
    const msgs = [
      userPrompt(
        "这个卡片怎么回事\n\nContext files:\n- /Users/me/.fleet/user-attachments/ab/paste-1.png",
      ),
    ];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "这个卡片怎么回事" });
  });

  it("leaves a prompt that merely mentions Context files mid-sentence alone", () => {
    const text = "Context files: 这个格式是谁定的？";
    expect(findLastUserInput([userPrompt(text)])).toEqual({ kind: "prompt", text });
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
  it("pairs every answer value with its question label", () => {
    expect(formatAnswer(JSON.stringify({ answers: { q1: "pick A", note: "also tidy up" } }))).toEqual(
      [
        { label: "q1", value: "pick A" },
        { label: "note", value: "also tidy up" },
      ],
    );
  });

  it("passes through non-JSON payloads like TASK FINISHED, unlabelled", () => {
    expect(formatAnswer("TASK FINISHED")).toEqual([{ label: "", value: "TASK FINISHED" }]);
  });
});

describe("answerLabel (mobile-web)", () => {
  it("keeps only the summary line before the --- separator", () => {
    expect(answerLabel("Done, waiting on you.\n---\nBoss, the **details**…")).toBe(
      "Done, waiting on you.",
    );
  });

  it("clips a long single-line question", () => {
    expect(answerLabel("x".repeat(80))).toBe(`${"x".repeat(48)}…`);
  });
});

describe("stripPromptEnvelope (mobile-web)", () => {
  it("drops slash-command envelopes", () => {
    expect(stripPromptEnvelope("<command-name>/loop</command-name>\nhand-written")).toBe(
      "hand-written",
    );
  });
});
