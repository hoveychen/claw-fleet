import { describe, expect, it } from "vitest";
import { filterMainRows, isSubagentTranscript } from "./mainRows";
import type { RawMessage } from "../types";

const row = (isSidechain: boolean): RawMessage =>
  ({ type: "assistant", isSidechain, message: { content: [{ type: "text", text: "hi" }] } }) as
    unknown as RawMessage;

const MAIN = "/Users/x/.claude/projects/-p/9bdf09d5.jsonl";
const SUB = "/Users/x/.claude/projects/-p/9bdf09d5/subagents/agent-a5b9297131f9d7bc4.jsonl";

describe("isSubagentTranscript", () => {
  it("recognises a subagent transcript by its subagents/ segment", () => {
    expect(isSubagentTranscript(SUB)).toBe(true);
    expect(isSubagentTranscript(MAIN)).toBe(false);
  });
});

describe("filterMainRows", () => {
  it("drops sidechain rows inlined into a main transcript", () => {
    expect(filterMainRows([row(false), row(true)], MAIN)).toHaveLength(1);
  });

  // Every line of a subagent's own file is isSidechain:true — filtering them
  // there rendered the whole drill-down as "暂无可显示的消息".
  it("keeps every row when the transcript itself is the subagent's", () => {
    expect(filterMainRows([row(true), row(true)], SUB)).toHaveLength(2);
  });
});
