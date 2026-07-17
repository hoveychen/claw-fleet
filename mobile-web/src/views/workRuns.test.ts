import { describe, expect, it } from "vitest";
import { countSteps, firstSentence, groupWorkRuns, isWorkRow, workRunTitle } from "./workRuns";
import { groupMetaRuns } from "./metaGrouping";
import type { ContentBlock, RawMessage } from "../types";

function assistant(content: ContentBlock[]): RawMessage {
  return { type: "assistant", message: { role: "assistant", content } };
}

function tool(name: string, input: Record<string, unknown> = {}): ContentBlock {
  return { type: "tool_use", id: `t-${name}-${Math.random()}`, name, input } as ContentBlock;
}

const think: ContentBlock = { type: "thinking", thinking: "hmm" } as ContentBlock;
const prose: ContentBlock = { type: "text", text: "Here is what I found." } as ContentBlock;

function user(text = "hi"): RawMessage {
  return { type: "user", message: { role: "user", content: [{ type: "text", text }] } };
}

describe("isWorkRow", () => {
  it("accepts pure thinking/tool records, rejects prose and decision tools", () => {
    expect(isWorkRow(assistant([think, tool("Bash")]))).toBe(true);
    expect(isWorkRow(assistant([think, prose]))).toBe(false);
    expect(isWorkRow(assistant([tool("AskUserQuestion")]))).toBe(false);
    expect(isWorkRow(assistant([tool("mcp__fleet__fleet__ask")]))).toBe(false);
    expect(isWorkRow(assistant([tool("request_user_input")]))).toBe(false);
    expect(isWorkRow(user())).toBe(false);
  });
});

describe("groupWorkRuns", () => {
  it("folds runs of ≥2 adjacent work rows, leaves singles alone", () => {
    const rows = [user(), assistant([think]), assistant([tool("Read")]), assistant([prose]), assistant([tool("Bash")])];
    const units = groupWorkRuns(groupMetaRuns(rows));
    expect(units.map((u) => u.kind)).toEqual(["single", "work-group", "single", "single"]);
    const group = units[1] as { msgs: RawMessage[] };
    expect(group.msgs.length).toBe(2);
  });
});

describe("countSteps / workRunTitle / firstSentence", () => {
  it("counts thinking + tool calls across the run", () => {
    expect(countSteps([assistant([think, tool("Bash")]), assistant([tool("Read")])])).toBe(3);
  });

  it("titles from the last thinking sentence, CJK-aware, clipped", () => {
    const msgs = [assistant([{ type: "thinking", thinking: "重新评估了去杠杆周期。承认之前判断过于乐观。" } as ContentBlock])];
    expect(workRunTitle(msgs)).toBe("重新评估了去杠杆周期。");
    expect(workRunTitle([assistant([tool("Bash")])])).toBe(null);
    expect(firstSentence("Reading release.yml to check")).toBe("Reading release.yml to check");
    expect(firstSentence("x".repeat(100)).length).toBe(64);
  });

  it("uses the last thinking block, not a mid-run one", () => {
    const msgs = [
      assistant([{ type: "thinking", thinking: "An early plan. Ignore me." } as ContentBlock, tool("Bash")]),
      assistant([{ type: "thinking", thinking: "Final intent here. Then more." } as ContentBlock, tool("Read")]),
    ];
    expect(workRunTitle(msgs)).toBe("Final intent here.");
  });
});
