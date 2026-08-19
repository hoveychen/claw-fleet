import { describe, expect, it } from "vitest";
import {
  AGENT_TOOL_CHOICES,
  detailPathForSession,
  tokenRequestFor,
  toolChoicesForSources,
  toolForAgentSource,
} from "./agentSource";
import type { SourceInfo } from "./useSourcesConfig";

const src = (name: string, on = true): SourceInfo => ({
  name,
  enabled: on,
  available: on,
});

describe("toolChoicesForSources", () => {
  it("dsh 源开着且装了时,新会话能选 dsh", () => {
    const tools = toolChoicesForSources([src("claude-code"), src("dsh")]).map(([v]) => v);
    expect(tools).toContain("dsh");
  });

  it("只列被监控的源", () => {
    const tools = toolChoicesForSources([src("claude-code"), src("codex", false)]).map(
      ([v]) => v,
    );
    expect(tools).toEqual(["claude"]);
  });

  it("配置未到 / 一个都没匹配上 → Claude-only,永不为空", () => {
    expect(toolChoicesForSources(null)).toEqual([AGENT_TOOL_CHOICES[0]]);
    expect(toolChoicesForSources([])).toEqual([AGENT_TOOL_CHOICES[0]]);
  });
});

describe("toolForAgentSource", () => {
  it("dsh 会话用 dsh 自己的清单,不是 Claude 的", () => {
    expect(toolForAgentSource("dsh")).toBe("dsh");
  });

  it("claude-code 注册名映射到裸 claude", () => {
    expect(toolForAgentSource("claude-code")).toBe("claude");
  });

  it("codex 保持 codex;认不出的 / 缺省的退回 claude", () => {
    expect(toolForAgentSource("codex")).toBe("codex");
    expect(toolForAgentSource(undefined)).toBe("claude");
    expect(toolForAgentSource("who-knows")).toBe("claude");
  });
});

describe("tokenRequestFor", () => {
  it("dsh 会话走 RPC 问用量,不拿 dsh:// 去读文件", () => {
    const r = tokenRequestFor({ agentSource: "dsh", jsonlPath: "dsh://abc" });
    expect(r.method).toBe("dsh_token_breakdown");
    expect(r.params).toEqual({ uri: "dsh://abc" });
  });

  it("其余源仍走读 jsonl 的那条", () => {
    const r = tokenRequestFor({
      agentSource: "claude-code",
      jsonlPath: "/x/s.jsonl",
      workspacePath: "/repo",
    });
    expect(r.method).toBe("token_breakdown");
    expect(r.params).toEqual({ path: "/x/s.jsonl", projectRoot: "/repo" });
  });
});

describe("detailPathForSession", () => {
  it("dsh 会话不拿 dsh:// 的 uri 去读 jsonl", () => {
    expect(detailPathForSession("dsh", "dsh://abc")).toBeUndefined();
  });

  it("codex 仍不可展开;claude 照常拿路径", () => {
    expect(detailPathForSession("codex", "/x/rollout.jsonl")).toBeUndefined();
    expect(detailPathForSession("claude-code", "/x/s.jsonl")).toBe("/x/s.jsonl");
    expect(detailPathForSession(undefined, "/x/s.jsonl")).toBe("/x/s.jsonl");
  });
});
