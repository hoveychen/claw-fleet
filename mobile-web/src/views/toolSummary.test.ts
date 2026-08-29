import { describe, expect, it } from "vitest";
import type { ContentBlock } from "../types";
import { decisionSummary, friendlyToolName, patchToolSummary, toolSummary } from "./toolSummary";

const tr = (key: string, ...args: Array<string | number>) => {
  let out = key;
  args.forEach((value, index) => {
    out = out.replace(`{${index}}`, String(value));
  });
  return out;
};

describe("friendlyToolName", () => {
  it("maps Fleet MCP ids to a readable label", () => {
    expect(friendlyToolName("mcp__fleet__fleet__ask", tr)).toBe("决策卡");
    expect(friendlyToolName("mcp__fleet__fleet__plan", tr)).toBe("计划");
    expect(friendlyToolName("fleet__watch", tr)).toBe("守望");
  });

  it("strips the mcp__server__ prefix of non-Fleet MCP tools", () => {
    expect(friendlyToolName("mcp__linear__create_issue", tr)).toBe("linear·create_issue");
  });

  it("passes a plain tool name through unchanged", () => {
    expect(friendlyToolName("AskUserQuestion", tr)).toBe("AskUserQuestion");
  });
});

describe("decisionSummary", () => {
  const block = (ask?: { q?: string; n?: number }) => ({
    type: "tool_use",
    name: "mcp__fleet__fleet__ask",
    _ask: ask,
  });

  it("shows the relay's gist beside the label", () => {
    expect(decisionSummary(block({ q: "查清楚了，缺的是渲染器。", n: 1 }), tr)).toBe(
      "决策卡 · 查清楚了，缺的是渲染器。",
    );
  });

  it("appends the question count on a multi-question card", () => {
    expect(decisionSummary(block({ q: "两件事要定", n: 3 }), tr)).toBe(
      "决策卡 · 两件事要定（3 题）",
    );
  });

  it("degrades to the bare label when the block carries no gist", () => {
    expect(decisionSummary(block(), tr)).toBe("决策卡");
    expect(decisionSummary(block({ q: "   " }), tr)).toBe("决策卡");
  });
});

describe("patchToolSummary", () => {
  it("names one added, updated, or deleted file without dumping the patch", () => {
    expect(patchToolSummary("*** Begin Patch\n*** Add File: src/new.ts\n*** End Patch", tr))
      .toBe("新建 new.ts");
    expect(patchToolSummary("*** Begin Patch\n*** Update File: src/old.ts\n*** End Patch", tr))
      .toBe("编辑 old.ts");
    expect(patchToolSummary("*** Begin Patch\n*** Delete File: src/gone.ts\n*** End Patch", tr))
      .toBe("删除 gone.ts");
  });

  it("counts multi-file patches", () => {
    const patch = `*** Begin Patch
*** Update File: a.ts
*** Add File: b.ts
*** Delete File: c.ts
*** End Patch`;
    expect(patchToolSummary(patch, tr)).toBe("编辑 3 个文件");
  });

  it("returns null for a non-patch command", () => {
    expect(patchToolSummary("const result = 1", tr)).toBeNull();
  });
});

describe("toolSummary", () => {
  const toolUse = (name: string, input: Record<string, unknown>) =>
    ({ type: "tool_use", id: "t1", name, input }) as ContentBlock;

  // An Agent call has no compact input field, so without `description` the chip
  // falls through to "" and every subagent in a session renders as the same
  // bare "Agent". Requires the relay to ship `description`
  // (TAIL_TOOL_INPUT_FIELDS in mobile_relay.rs).
  it("labels an Agent call with its task description", () => {
    expect(toolSummary(toolUse("Agent", { description: "审计 issuer 读取点" }))).toBe(
      "审计 issuer 读取点",
    );
  });

  it("prefers the description over the raw command for shells", () => {
    const input = { description: "Show working tree status", command: "git status --short" };
    expect(toolSummary(toolUse("Bash", input))).toBe("Show working tree status");
    // PowerShell is the Windows shell tool, same shape — the desktop
    // ToolUseBlock treats it identically and so must the phone.
    expect(toolSummary(toolUse("PowerShell", input))).toBe("Show working tree status");
  });

  it("falls back to the command when a transcript recorded no description", () => {
    expect(toolSummary(toolUse("Bash", { command: "git status --short" }))).toBe(
      "git status --short",
    );
    expect(toolSummary(toolUse("PowerShell", { command: "Get-ChildItem" }))).toBe("Get-ChildItem");
  });

  it("ignores a blank description rather than showing an empty chip", () => {
    expect(toolSummary(toolUse("Agent", { description: "   " }))).toBe("");
    expect(toolSummary(toolUse("Bash", { description: "  ", command: "ls" }))).toBe("ls");
  });

  it("still labels non-shell tools from their own fields", () => {
    expect(toolSummary(toolUse("Read", { file_path: "/a/b.rs" }))).toBe("/a/b.rs");
  });
});
