// 每个 agent 源在移动端的分支判断，集中一处。
//
// 这些判断原先是散在组件里的 `agentSource === "codex"` 三元式，写它们的时候
// Fleet 只有 claude/codex 两个源；dsh 接进来之后每一处都默默把 dsh 当成 Claude
// ——给 dsh 会话列 Claude 的模型、拿 `dsh://` 的 uri 去读 jsonl。抽成纯函数是
// 为了能被测试逐个钉住，新增第四个源时也只改这一个文件。
//
// 桌面端的对应物是 claw-fleet-desktop/app/modelChoices.ts（toolForAgentSource /
// tokenPanelForAgentSource）。
import type { SourceInfo } from "./useSourcesConfig";

/** Fleet 能拉起新会话的 agent 工具（镜像桌面端 AGENT_TOOL_CHOICES）。
 *  值是 launcher 的 tool 值：Claude 源注册名是 "claude-code"，但 tool 值是裸的
 *  "claude"；codex / dsh 注册名与 tool 值同名。 */
export const AGENT_TOOL_CHOICES: Array<[string, string]> = [
  ["claude", "Claude"],
  ["codex", "Codex"],
  ["dsh", "dsh"],
];

/** 源注册名 → launcher 的 tool 值。 */
function sourceNameToTool(name: string): string {
  return name === "claude-code" ? "claude" : name;
}

/** 把工具选择器限制在真正被监控的源上（源开着 **且** 主机上装了）。
 *  `null`（配置还没到）或一个都没匹配上时退回 Claude-only，这样选择器永远不为空、
 *  也不会先闪一个没在监控的工具再藏起来。 */
export function toolChoicesForSources(
  sources: SourceInfo[] | null,
): Array<[string, string]> {
  const active = new Set(
    (sources ?? []).filter((s) => s.enabled && s.available).map((s) => sourceNameToTool(s.name)),
  );
  const filtered = AGENT_TOOL_CHOICES.filter(([v]) => active.has(v));
  return filtered.length ? filtered : [AGENT_TOOL_CHOICES[0]];
}

/** 会话的 `agentSource` 对应哪套模型/effort 清单。Fleet 不做兜底猜测：认不出的
 *  源按 Claude 处理，那是注册表自己的 fallback。 */
export function toolForAgentSource(agentSource: string | undefined | null): string {
  const tool = sourceNameToTool((agentSource ?? "").trim());
  return AGENT_TOOL_CHOICES.some(([v]) => v === tool) ? tool : "claude";
}

/** 工具行能不能展开详情 —— 展开是拿 tool_use_id 去读 Claude 的 jsonl。
 *
 *  codex 的 rollout 没有 toolUseResult，折叠格式也让扫描失效；dsh 干脆没有
 *  transcript 文件，它的 `jsonlPath` 是个 `dsh://<id>` 的 uri，拿去当路径读只会
 *  失败。两者都返回 undefined = 工具行不可展开。 */
export function detailPathForSession(
  agentSource: string | undefined | null,
  jsonlPath: string | undefined,
): string | undefined {
  return toolForAgentSource(agentSource) === "claude" ? jsonlPath : undefined;
}

/** Token 页签该向 relay 要哪个方法。
 *
 *  每个源用自己的词汇记 token、走自己的通道,面板并不通用:Claude 的从会话
 *  JSONL 里解析出来(`token_breakdown`),dsh 压根没有 transcript 文件,它的
 *  用量要拿 `dsh://<id>` 的 uri 走 RPC 问(`dsh_token_breakdown`)——把 uri 当
 *  路径喂给读文件的那条,只会稳定地显示「分析失败」。 */
export function tokenRequestFor(session: {
  agentSource?: string;
  jsonlPath?: string;
  workspacePath?: string;
}): { method: string; params: Record<string, unknown> } {
  if (toolForAgentSource(session.agentSource) === "dsh") {
    return { method: "dsh_token_breakdown", params: { uri: session.jsonlPath } };
  }
  return {
    method: "token_breakdown",
    params: { path: session.jsonlPath, projectRoot: session.workspacePath },
  };
}
