import { describe, it, expect } from "vitest";
import { answerLabel, findLastUserInput, formatAnswer, oneLineSnippet, shouldAutoExpand, stripPromptEnvelope } from "./useLastUserInput";
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

  it("returns the previous card's answer with a clipped question label", () => {
    const msgs = [
      userPrompt("开始吧"),
      askCall("t1", "mcp__fleet__fleet__ask"),
      askResult(
        "t1",
        JSON.stringify({ answers: { "两条路线选一条。\n---\n很长很长的问题正文…": "先做 A 方案" } }),
      ),
      assistantText("好的，我按 A 方案来。"),
    ];
    expect(findLastUserInput(msgs)).toEqual({
      kind: "answer",
      text: "先做 A 方案",
      answers: [{ label: "两条路线选一条。", value: "先做 A 方案" }],
    });
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
    const msgs = [userPrompt("看下这张截图"), assistantText("读一下"), imageCompanion];
    expect(findLastUserInput(msgs)).toEqual({ kind: "prompt", text: "看下这张截图" });
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
  it("pairs every answer value with its question label", () => {
    const raw = JSON.stringify({ answers: { q1: "选 A", rollout_note: "顺便清理一下" } });
    expect(formatAnswer(raw)).toEqual([
      { label: "q1", value: "选 A" },
      { label: "rollout_note", value: "顺便清理一下" },
    ]);
  });

  it("passes through non-JSON payloads like TASK FINISHED, unlabelled", () => {
    expect(formatAnswer("TASK FINISHED")).toEqual([{ label: "", value: "TASK FINISHED" }]);
  });
});

describe("answerLabel", () => {
  it("keeps only the TTS summary line before the --- separator", () => {
    expect(answerLabel("改完了，等你放行。\n---\n老板，**详细**报告……")).toBe("改完了，等你放行。");
  });

  it("clips a long single-line question", () => {
    expect(answerLabel("x".repeat(80))).toBe(`${"x".repeat(48)}…`);
  });
});

describe("stripPromptEnvelope", () => {
  it("drops slash-command envelopes", () => {
    expect(
      stripPromptEnvelope("<command-name>/loop</command-name>\n手写内容"),
    ).toBe("手写内容");
  });
});

describe("oneLineSnippet", () => {
  it("collapses a multi-line answer into one line", () => {
    expect(oneLineSnippet("先修 P3\n\n再合并")).toBe("先修 P3 再合并");
  });

  it("drops fenced code bodies and image syntax", () => {
    expect(oneLineSnippet("看这个\n```js\nconst a = 1;\n```\n![shot](data:image/png;base64,AAAA)")).toBe(
      "看这个",
    );
  });

  it("keeps link text but not the URL", () => {
    expect(oneLineSnippet("见 [文档](https://example.com/a/b)")).toBe("见 文档");
  });

  it("clips a wall of text", () => {
    expect(oneLineSnippet("x".repeat(300))).toBe(`${"x".repeat(200)}…`);
  });
});

describe("shouldAutoExpand", () => {
  it("opens a short single answer", () => {
    expect(shouldAutoExpand([{ label: "", value: "先修 P3 再合并" }])).toBe(true);
  });

  it("stays collapsed for a long one", () => {
    expect(shouldAutoExpand([{ label: "", value: "长".repeat(200) }])).toBe(false);
  });

  it("stays collapsed once there is more than one answer", () => {
    expect(
      shouldAutoExpand([
        { label: "q1", value: "是" },
        { label: "q2", value: "否" },
      ]),
    ).toBe(false);
  });

  it("measures the flattened form, not the raw markdown", () => {
    const value = `看这个\n\`\`\`js\n${"const a = 1;\n".repeat(20)}\`\`\``;
    expect(shouldAutoExpand([{ label: "", value }])).toBe(true);
  });
});
