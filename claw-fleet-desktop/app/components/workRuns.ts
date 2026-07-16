import type { RawMessage, TextBlock, ToolUseBlock } from "../types";
import { isDecisionTool } from "../toolResults";
import { dayKey } from "../messageRows";
import type { MetaRenderUnit } from "./metaGrouping";

/**
 * Rows that a `WorkRunBlock` folds into one collapsed band — assistant records
 * whose visible content is pure scaffolding: thinking and tool calls, no prose.
 *
 * A record carrying any non-empty `text` block is where the agent spoke to the
 * reader; it must stay a full-width row. Decision tools (AskUserQuestion,
 * fleet__ask, …) are where the conversation turned, so a record carrying one is
 * never swept into a band either, even though it is technically a tool call.
 */
export function isWorkRow(msg: RawMessage): boolean {
  if (msg.type !== "assistant" || !msg.message) return false;
  const content = msg.message.content;
  if (!Array.isArray(content) || content.length === 0) return false;
  let sawWork = false;
  for (const block of content) {
    if (block.type === "text") {
      if ((block as TextBlock).text.trim()) return false;
      continue;
    }
    if (block.type === "thinking" || block.type === "redacted_thinking") {
      sawWork = true;
      continue;
    }
    if (block.type === "tool_use") {
      if (isDecisionTool((block as ToolUseBlock).name)) return false;
      sawWork = true;
      continue;
    }
    // An unknown block renders as itself (or not at all) — don't hide it.
    return false;
  }
  return sawWork;
}

export type RenderUnit =
  | MetaRenderUnit
  | { kind: "work-group"; startLocal: number; msgs: RawMessage[] };

/**
 * Second grouping pass over `groupMetaRuns` output: collapse runs of adjacent
 * work rows into `work-group` units. The two passes can't fight over a row —
 * meta groups fold synthetic *user* turns, work groups fold *assistant* turns.
 *
 * Same invariants as meta grouping: a band needs at least two rows (a lone
 * tool-call record reads fine as a card), and runs break at day boundaries so
 * the day-separator invariant holds.
 */
export function groupWorkRuns(units: MetaRenderUnit[]): RenderUnit[] {
  const out: RenderUnit[] = [];
  for (let k = 0; k < units.length; ) {
    const unit = units[k];
    if (unit.kind === "single" && isWorkRow(unit.msg)) {
      const day = dayKey(unit.msg.timestamp);
      let j = k + 1;
      while (j < units.length) {
        const next = units[j];
        if (next.kind !== "single" || !isWorkRow(next.msg)) break;
        if (dayKey(next.msg.timestamp) !== day) break;
        j++;
      }
      if (j - k >= 2) {
        out.push({
          kind: "work-group",
          startLocal: unit.startLocal,
          msgs: units.slice(k, j).map((u) => (u as { msg: RawMessage }).msg),
        });
        k = j;
        continue;
      }
    }
    out.push(unit);
    k++;
  }
  return out;
}

// ── Band label material ───────────────────────────────────────────────────────

/** Everything on a band is countable fact except the category, which is a
 *  transparent rule over the run's tool names (see `runCategory`). */
export interface WorkRunSummary {
  category: WorkRunCategory;
  /** Tool calls + thinking segments across the run. */
  steps: number;
  /** Tool name → call count, insertion-ordered; thinking counts under "thinking". */
  toolCounts: Array<[string, number]>;
  /** Summed `usage.output_tokens` of the folded records. */
  outputTokens: number;
}

export type WorkRunCategory = "edit" | "run" | "web" | "explore" | "think" | "work";

const EDIT_TOOLS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit"]);
const WEB_TOOLS = new Set(["WebSearch", "WebFetch"]);
const EXPLORE_TOOLS = new Set(["Read", "Grep", "Glob", "Explore", "LSP", "TodoWrite", "TodoRead"]);

/**
 * Category precedence: what the run *changed* outranks what it merely looked
 * at. A run that edits anything is an edit run even if it read ten files first;
 * commands outrank reads for the same reason. Reads/searches only label the
 * run when nothing stronger appears, and a tool outside every set (Agent,
 * Skill, Workflow, …) falls through to the generic "work".
 */
export function runCategory(toolNames: string[]): WorkRunCategory {
  if (toolNames.length === 0) return "think";
  if (toolNames.some((n) => EDIT_TOOLS.has(n))) return "edit";
  if (toolNames.some((n) => n === "Bash")) return "run";
  if (toolNames.every((n) => WEB_TOOLS.has(n))) return "web";
  if (toolNames.every((n) => EXPLORE_TOOLS.has(n) || WEB_TOOLS.has(n))) return "explore";
  return "work";
}

export function summarizeWorkRun(msgs: RawMessage[]): WorkRunSummary {
  const toolNames: string[] = [];
  const counts = new Map<string, number>();
  let thinking = 0;
  let outputTokens = 0;
  for (const msg of msgs) {
    outputTokens += msg.message?.usage?.output_tokens ?? 0;
    const content = msg.message?.content;
    if (!Array.isArray(content)) continue;
    for (const block of content) {
      if (block.type === "thinking" || block.type === "redacted_thinking") {
        thinking++;
        continue;
      }
      if (block.type === "tool_use") {
        const name = (block as ToolUseBlock).name;
        toolNames.push(name);
        counts.set(name, (counts.get(name) ?? 0) + 1);
      }
    }
  }
  const toolCounts = [...counts.entries()];
  if (thinking > 0) toolCounts.push(["thinking", thinking]);
  return {
    category: runCategory(toolNames),
    steps: toolNames.length + thinking,
    toolCounts,
    outputTokens,
  };
}
