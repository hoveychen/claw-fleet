// Shared agent-mail parser (shared-ts/agentMail.ts). The footer shape must match
// `with_reply_address` in claw-fleet-core/src/mcp_inspect.rs byte for byte.
import { describe, expect, it } from "vitest";
import { agentMailPlainText, splitAgentMail } from "../../shared-ts/agentMail";

const ID = "a58442bc-62d4-421f-9253-0d71f6c5469f";
const footer = (id: string, ws?: string) =>
  `\n\n---\n（这条消息来自 Fleet 会话 ${ws ? `${id}（${ws}）` : id}。要回复它，用 ` +
  `\`fleet__control action=send id=${id} text=<回复>\`（CLI：\`fleet send ${id} <回复>\`）。）`;

describe("splitAgentMail", () => {
  it("returns null for a turn without a footer", () => {
    expect(splitAgentMail("进度怎么样？")).toBeNull();
    expect(splitAgentMail("a\n\n---\nb")).toBeNull();
  });

  it("lifts the sender and workspace out of the footer", () => {
    const parts = splitAgentMail(`请接着做：把策略写成 rust 版。\n\n第二段。${footer(ID, "claude-fleet")}`);
    expect(parts).toEqual([
      { body: "请接着做：把策略写成 rust 版。\n\n第二段。", from: { id: ID, workspace: "claude-fleet" } },
    ]);
  });

  it("accepts a sender that was not scanned yet", () => {
    expect(splitAgentMail(`hi${footer("dddddddd-4444")}`)).toEqual([
      { body: "hi", from: { id: "dddddddd-4444" } },
    ]);
  });

  it("splits queued messages joined into one prompt", () => {
    const text = `first${footer(ID, "a")}\n\nsecond${footer("bbbb", "b")}\n\n老板自己补的一句`;
    const parts = splitAgentMail(text)!;
    expect(parts.map((p) => [p.body, p.from?.id])).toEqual([
      ["first", ID],
      ["second", "bbbb"],
      ["老板自己补的一句", undefined],
    ]);
    expect(agentMailPlainText(parts)).toBe("first\n\nsecond\n\n老板自己补的一句");
  });
});
