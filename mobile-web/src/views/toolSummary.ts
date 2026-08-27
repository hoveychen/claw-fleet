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
  set_session_title: "设置标题",
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

  if (block.name === "Bash" || block.name === "Agent") {
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

  for (const field of TOOL_SUMMARY_FIELDS) {
    const value = input[field];
    if (typeof value === "string" && value) return value;
  }
  return "";
}
