import { describe, it, expect } from "vitest";
import { groupWorkRuns, isWorkRow, runCategory, summarizeWorkRun } from "./workRuns";
import { groupMetaRuns } from "./metaGrouping";
import type { ContentBlock, RawMessage } from "../types";

const DAY1 = "2026-07-16T10:00:00.000Z";
const DAY2 = "2026-07-17T10:00:00.000Z";

function assistant(content: ContentBlock[], ts = DAY1, outputTokens = 0): RawMessage {
  return {
    type: "assistant",
    timestamp: ts,
    message: {
      role: "assistant",
      content,
      usage: { input_tokens: 1, output_tokens: outputTokens },
    },
  };
}

function tool(name: string, input: Record<string, unknown> = {}): ContentBlock {
  return { type: "tool_use", id: `t-${name}-${Math.floor(Math.random() * 1e9)}`, name, input };
}

const think: ContentBlock = { type: "thinking", thinking: "hmm" };
const prose: ContentBlock = { type: "text", text: "Here is what I found." };
const emptyText: ContentBlock = { type: "text", text: "  " };

function user(ts = DAY1): RawMessage {
  return { type: "user", timestamp: ts, message: { role: "user", content: "hi" } };
}

describe("isWorkRow", () => {
  it("accepts assistant records with only thinking and tool calls", () => {
    expect(isWorkRow(assistant([think]))).toBe(true);
    expect(isWorkRow(assistant([tool("Bash")]))).toBe(true);
    expect(isWorkRow(assistant([think, tool("Read")]))).toBe(true);
  });

  it("rejects records carrying prose, keeps ones with only blank text", () => {
    expect(isWorkRow(assistant([prose, tool("Bash")]))).toBe(false);
    expect(isWorkRow(assistant([emptyText, tool("Bash")]))).toBe(true);
  });

  it("rejects decision-tool records and non-assistant rows", () => {
    expect(isWorkRow(assistant([tool("AskUserQuestion")]))).toBe(false);
    expect(isWorkRow(user())).toBe(false);
    expect(isWorkRow(assistant([]))).toBe(false);
  });
});

describe("groupWorkRuns", () => {
  const wrap = (msgs: RawMessage[]) => groupWorkRuns(groupMetaRuns(msgs));

  it("folds adjacent work rows into one work-group", () => {
    const msgs = [assistant([think]), assistant([tool("Bash")]), assistant([tool("Read")])];
    const units = wrap(msgs);
    expect(units).toHaveLength(1);
    expect(units[0]).toMatchObject({ kind: "work-group", startLocal: 0 });
    expect((units[0] as { msgs: RawMessage[] }).msgs).toHaveLength(3);
  });

  it("leaves a lone work row as a single", () => {
    const units = wrap([assistant([tool("Bash")])]);
    expect(units.map((u) => u.kind)).toEqual(["single"]);
  });

  it("breaks runs on prose rows and preserves startLocal", () => {
    const msgs = [
      user(),
      assistant([think]),
      assistant([tool("Bash")]),
      assistant([prose]),
      assistant([tool("Read")]),
    ];
    const units = wrap(msgs);
    expect(units.map((u) => u.kind)).toEqual(["single", "work-group", "single", "single"]);
    expect(units[1]).toMatchObject({ startLocal: 1 });
    expect(units[2]).toMatchObject({ startLocal: 3 });
    expect(units[3]).toMatchObject({ startLocal: 4 });
  });

  it("breaks a run at a day boundary", () => {
    const msgs = [
      assistant([think], DAY1),
      assistant([tool("Bash")], DAY1),
      assistant([think], DAY2),
      assistant([tool("Bash")], DAY2),
    ];
    const units = wrap(msgs);
    expect(units).toHaveLength(2);
    expect(units.every((u) => u.kind === "work-group")).toBe(true);
  });

  it("passes meta-groups through untouched", () => {
    const meta = (ts = DAY1): RawMessage => ({ type: "user", isMeta: true, timestamp: ts });
    const msgs = [meta(), meta(), assistant([think]), assistant([tool("Bash")])];
    const units = wrap(msgs);
    expect(units.map((u) => u.kind)).toEqual(["meta-group", "work-group"]);
  });
});

describe("runCategory", () => {
  it("ranks edits above commands above lookups", () => {
    expect(runCategory(["Read", "Bash", "Edit"])).toBe("edit");
    expect(runCategory(["Read", "Bash"])).toBe("run");
    expect(runCategory(["Read", "Grep", "WebFetch"])).toBe("explore");
    expect(runCategory(["WebSearch", "WebFetch"])).toBe("web");
    expect(runCategory([])).toBe("think");
    expect(runCategory(["Agent", "Read"])).toBe("work");
  });
});

describe("summarizeWorkRun", () => {
  it("counts steps, tools, thinking and output tokens across the run", () => {
    const msgs = [
      assistant([think, tool("Bash")], DAY1, 100),
      assistant([tool("Bash")], DAY1, 20),
      assistant([tool("Read")], DAY1, 3),
    ];
    const s = summarizeWorkRun(msgs);
    expect(s.category).toBe("run");
    expect(s.steps).toBe(4);
    expect(s.toolCounts).toEqual([
      ["Bash", 2],
      ["Read", 1],
      ["thinking", 1],
    ]);
    expect(s.outputTokens).toBe(123);
  });
});
