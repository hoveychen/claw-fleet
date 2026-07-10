/**
 * Which transcript records become a visible row, and how those rows group by
 * day.
 *
 * `MessageRow` calls `isRenderableRow` rather than re-deriving the condition,
 * so the list's bookkeeping (the render window, the day separators, the search
 * indices) can never disagree with what actually reaches the DOM.
 */

import type { RawMessage } from "./types";

/**
 * Whether this record renders anything.
 *
 * A user record whose content holds only `tool_result` blocks is not a turn the
 * user took — it is a tool's output, already rendered inside that tool's card.
 * These are 32% of all user/assistant records in a tool-heavy session, so
 * counting them would make a "100 message" window show ~67 rows and would strand
 * day separators above nothing.
 */
export function isRenderableRow(msg: RawMessage): boolean {
  if (msg.type !== "user" && msg.type !== "assistant") return false;
  if (!msg.message) return false;
  if (msg.type === "user" && !msg.isCompactSummary) {
    const content = msg.message.content;
    if (Array.isArray(content) && !content.some((b) => b.type !== "tool_result")) {
      return false;
    }
  }
  return true;
}

/**
 * The prose of a message, as text worth putting on the clipboard.
 *
 * Assistant turns keep their markdown source — that is what a reader wants to
 * paste into an issue or a doc. Reasoning, tool calls and tool output are left
 * out: they are scaffolding, and the tool cards already offer their own bodies.
 * Images become a placeholder rather than a wall of base64.
 *
 * Returns `""` when a message carries no prose at all (a turn that only called
 * tools), which callers use to decide whether a copy control makes sense.
 */
export function messageToText(msg: RawMessage): string {
  const content = msg.message?.content;
  if (typeof content === "string") return content.trim();
  if (!Array.isArray(content)) return "";

  const parts: string[] = [];
  for (const block of content) {
    if (block.type === "text") {
      parts.push((block as { type: "text"; text: string }).text);
    } else if (block.type === "image") {
      parts.push("[image]");
    }
  }
  return parts.join("\n\n").trim();
}

/** Local-time `YYYY-MM-DD` for a record, or null when it has no usable stamp. */
export function dayKey(timestamp: string | undefined): string | null {
  if (!timestamp) return null;
  const d = new Date(timestamp);
  if (Number.isNaN(d.getTime())) return null;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/** Offset in days from `today` — 0 today, 1 yesterday, ... Null if unparseable. */
export function daysAgo(key: string, today: Date): number | null {
  const parts = key.split("-").map(Number);
  if (parts.length !== 3 || parts.some(Number.isNaN)) return null;
  const [y, m, d] = parts;
  const then = new Date(y, m - 1, d);
  const now = new Date(today.getFullYear(), today.getMonth(), today.getDate());
  return Math.round((now.getTime() - then.getTime()) / 86_400_000);
}
