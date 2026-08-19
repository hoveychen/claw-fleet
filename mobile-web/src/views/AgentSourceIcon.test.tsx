import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { AgentSourceIcon } from "./AgentSourceIcon";

const html = (source?: string) =>
  renderToStaticMarkup(<AgentSourceIcon source={source} />);

describe("AgentSourceIcon", () => {
  it("dsh 会话画 dsh 自己的标记，而不是退回 Claude", () => {
    expect(html("dsh")).not.toBe(html("claude-code"));
  });

  it("未知 / 缺省 source 仍退回 Claude 标记", () => {
    expect(html(undefined)).toBe(html("claude-code"));
    expect(html("who-knows")).toBe(html("claude-code"));
  });

  it("codex 仍是 codex 的标记", () => {
    expect(html("codex")).not.toBe(html("claude-code"));
    expect(html("codex")).not.toBe(html("dsh"));
  });
});
