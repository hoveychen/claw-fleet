import { describe, expect, it } from "vitest";
import { decisionSummary, friendlyToolName, patchToolSummary } from "./toolSummary";

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
