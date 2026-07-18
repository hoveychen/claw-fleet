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

  for (const field of TOOL_SUMMARY_FIELDS) {
    const value = input[field];
    if (typeof value === "string" && value) return value;
  }
  return "";
}
