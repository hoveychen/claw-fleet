import { t } from "../i18n";
import type { ContentBlock } from "../types";

type Translate = (key: string, ...args: Array<string | number>) => string;

interface PatchFile {
  op: "Add" | "Update" | "Delete";
  path: string;
}

const TOOL_SUMMARY_FIELDS = ["command", "file_path", "pattern", "path", "query", "url", "skill"];

function basename(path: string): string {
  const parts = path.split(/[/\\]/);
  return parts[parts.length - 1] || path;
}

function parsePatchFiles(patch: string): PatchFile[] {
  const files: PatchFile[] = [];
  const re = /^\*\*\* (Add|Update|Delete) File: (.+)$/gm;
  let match: RegExpExecArray | null;
  while ((match = re.exec(patch)) !== null) {
    files.push({ op: match[1] as PatchFile["op"], path: match[2].trim() });
  }
  return files;
}

/** Compact mobile label for the V4A body reconstructed from patch_apply_end. */
export function patchToolSummary(command: string, tr: Translate = t): string | null {
  const files = parsePatchFiles(command);
  if (files.length === 0) return null;
  if (files.length > 1) return tr("编辑 {0} 个文件", files.length);

  const file = files[0];
  const action = file.op === "Add" ? "新建 {0}" : file.op === "Delete" ? "删除 {0}" : "编辑 {0}";
  return tr(action, basename(file.path));
}

/**
 * Human-readable label (i18n source string) for each Fleet MCP tool, keyed by
 * the tail of its wire name (`mcp__fleet__fleet__<tail>`). Mirrors the desktop
 * `FLEET_TOOL_LABEL_KEYS` map.
 */
const FLEET_TOOL_LABELS: Record<string, string> = {
  ask: "决策卡",
  render_a2ui: "富交互卡",
  plan: "计划",
  handoff: "交接",
  watch: "守望",
  loop: "循环",
  schedule: "定时",
  wiki: "知识库",
  artifact: "产出",
  inspect: "巡检",
  control: "信号",
  notes: "笔记",
  history: "历史",
  set_session_title: "设置标题",
  image: "生成图片",
  image_edit: "修改图片",
  permission_prompt: "权限询问",
};

/**
 * Friendly label for a raw tool id in a ToolSearch `select:` list. Fleet MCP
 * tools (`mcp__fleet__fleet__ask`, …) map to a translated label; any other MCP
 * tool (`mcp__<server>__<tool>`) drops the `mcp__server__` prefix and renders
 * `server·tool`; a plain tool name passes through unchanged. Mirrors the
 * desktop `friendlyToolName`.
 */
export function friendlyToolName(rawId: string, tr: Translate = t): string {
  const id = rawId.trim();
  for (const [tail, label] of Object.entries(FLEET_TOOL_LABELS)) {
    if (id === `fleet__${tail}` || id.endsWith(`fleet__fleet__${tail}`)) return tr(label);
  }
  if (id.startsWith("mcp__")) {
    const parts = id.split("__");
    if (parts.length >= 3) return `${parts[1]}·${parts.slice(2).join("__")}`;
  }
  return id;
}

/**
 * Collapsed rail line for a decision card. The card body never reaches the
 * phone (the relay's input whitelist drops `questions`), so the gist rides on
 * the block as `_ask` — without it every decision chip in a session reads the
 * same bare 「决策卡」 and the reader cannot tell which question was which.
 */
export function decisionSummary(block: ContentBlock, tr: Translate = t): string {
  const label = tr("决策卡");
  const gist = block._ask?.q?.trim();
  const count = block._ask?.n ?? 0;
  const head = gist ? `${label} · ${gist}` : label;
  return count > 1 ? tr("{0}（{1} 题）", head, count) : head;
}

/** The readable one-line label shown beside a mobile transcript tool icon. */
export function toolSummary(block: ContentBlock): string {
  const input = block.input;
  if (input === undefined) return "";

  // Bash / PowerShell / Agent all carry a model-written `description`: a
  // one-line summary for the shells, a 3-5 word task name for Agent. Prefer it
  // over the fallback loop below — a raw shell command is often a long escaped
  // grep, and Agent has no compact field at all. Mirrors the desktop
  // `ToolUseBlock`, including PowerShell (the Windows shell tool, same
  // `description`/`command` shape as Bash).
  if (block.name === "Bash" || block.name === "PowerShell" || block.name === "Agent") {
    const description = input.description;
    if (typeof description === "string" && description.trim()) return description.trim();
  }

  if (block.name === "apply_patch") {
    const command = input.command;
    if (typeof command === "string") {
      const summary = patchToolSummary(command);
      if (summary) return summary;
    }
  }

  // ToolSearch loads deferred tool schemas; its raw query ("select:AskUserQuestion",
  // "notebook jupyter") is opaque, so relabel it the same way the desktop does.
  if (block.name === "ToolSearch") {
    const query = typeof input.query === "string" ? input.query.trim() : "";
    if (query) {
      const sel = query.match(/^select:(.*)$/i);
      if (sel) {
        const names = sel[1]
          .split(",")
          .map((s) => s.trim())
          .filter(Boolean)
          .map((n) => friendlyToolName(n))
          .join(", ");
        if (names) return t("加载工具 {0}", names);
      } else {
        return t("搜索工具：{0}", query);
      }
    }
  }

  const dsh = dshToolSummary(block.name, input);
  if (dsh) return dsh;

  for (const field of TOOL_SUMMARY_FIELDS) {
    const value = input[field];
    if (typeof value === "string" && value) return value;
  }
  return "";
}

/**
 * dsh's tools with no Claude counterpart. They keep their own names through
 * `dsh_messages.rs`, and none of their inputs carries a `TOOL_SUMMARY_FIELDS`
 * key — so without this every one of them rendered as a bare icon with no text
 * at all. Mirrors the desktop's `dshToolSummary`.
 */
function dshToolSummary(name: string | undefined, input: Record<string, unknown>): string {
  const first = (key: string) => {
    const v = input[key];
    if (typeof v !== "string") return "";
    const line = v.trim().split("\n", 1)[0].trim();
    return line.length > 60 ? `${line.slice(0, 59)}…` : line;
  };
  switch (name) {
    case "job_output":
      return t("读取后台任务输出");
    case "job_kill":
      return t("停止后台任务");
    case "job_list":
      return t("列出后台任务");
    case "terminal_open":
      return t("打开常驻终端");
    case "terminal_list":
      return t("列出常驻终端");
    case "terminal_read":
      return t("读取终端输出");
    case "terminal_send": {
      const text = first("text");
      return text ? t("输入：{0}", text) : t("向终端发送输入");
    }
    case "terminal_close":
      return t("关闭常驻终端");
    case "terminal_signal":
      return t("向终端发送信号");
    case "send_message": {
      const msg = first("message");
      return msg ? t("发给子代理：{0}", msg) : t("给子代理发消息");
    }
    case "list_agents":
      return t("列出子代理");
    case "interrupt_agent":
      return t("打断子代理");
    case "report": {
      const out = first("output");
      return out ? t("向上级汇报：{0}", out) : t("向上级汇报");
    }
    case "create_goal":
    case "update_goal":
      return t("更新目标");
    case "get_goal":
      return t("读取目标");
    case "schedule_create":
    case "schedule_delete":
    case "schedule_list":
      return t("管理定时任务");
    case "session_search":
    case "session_trace":
    case "session_event_read":
    case "session_event_search":
    case "session_event_trace":
      return t("检索会话记录");
    default:
      return "";
  }
}
