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

/**
 * Fields a tool card shows on its collapsed header — the same list
 * `ToolUseBlock.formatInput` picks from.
 */
const TOOL_SUMMARY_FIELDS = ["command", "file_path", "pattern", "path", "query", "url"] as const;

/**
 * Lowercased haystack for searching one message.
 *
 * Covers prose, reasoning, and the one input field each tool card prints on its
 * collapsed header — a file path, a shell command, a URL. Two deliberate
 * exclusions:
 *
 * - **The rest of the tool input.** A hit buried in a JSON blob would scroll the
 *   reader to a row showing nothing that matches, which is worse than no hit.
 *   Everything indexed here is legible without expanding a card.
 * - **The tool's name.** Tool names are ordinary English words — Read, Write,
 *   Edit, Task, Bash — so indexing them buries real hits: searching "bash" over
 *   a sample of 12,806 messages jumped from 58 matches to 3,439, one per shell
 *   call, and "write the test" would match every file write.
 *
 * Built once per message, so a search over a 2000-message session doesn't
 * re-serialise every tool input on each keystroke.
 */
export function messageSearchText(msg: RawMessage): string {
  const content = msg.message?.content;
  if (typeof content === "string") return content.toLowerCase();
  if (!Array.isArray(content)) return "";

  const parts: string[] = [];
  for (const block of content) {
    if (block.type === "text") {
      parts.push((block as { type: "text"; text: string }).text);
    } else if (block.type === "thinking") {
      parts.push((block as { type: "thinking"; thinking: string }).thinking);
    } else if (block.type === "tool_use") {
      const tu = block as { input?: Record<string, unknown> };
      for (const field of TOOL_SUMMARY_FIELDS) {
        const v = tu.input?.[field];
        if (typeof v === "string") {
          parts.push(v);
          break; // formatInput shows the first match only
        }
      }
    }
  }
  return parts.join(" ").toLowerCase();
}

/** Indices of every message matching all search terms. */
export function findMatches(haystacks: string[], terms: string[]): number[] {
  if (terms.length === 0) return [];
  const out: number[] = [];
  for (let i = 0; i < haystacks.length; i++) {
    if (terms.every((t) => haystacks[i].includes(t))) out.push(i);
  }
  return out;
}

/** Local-time `YYYY-MM-DD` for a record, or null when it has no usable stamp. */
export function dayKey(timestamp: string | undefined): string | null {
  if (!timestamp) return null;
  const d = new Date(timestamp);
  if (Number.isNaN(d.getTime())) return null;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}`;
}

/**
 * Display form of a transcript model id: `claude-opus-4-8` → `opus 4.8`,
 * `claude-haiku-4-5-20251001` → `haiku 4.5`, `claude-sonnet-5` → `sonnet 5`.
 * Ids outside the claude naming scheme (`gpt-5.6-sol`) pass through unchanged.
 */
export function shortModelName(model: string): string {
  const m = model.replace(/^claude-/, "").replace(/-\d{8}$/, "");
  const twoPart = m.match(/^(.*?)-(\d+)-(\d+)$/);
  if (twoPart) return `${twoPart[1]} ${twoPart[2]}.${twoPart[3]}`;
  const onePart = m.match(/^(.*?)-(\d+)$/);
  if (onePart) return `${onePart[1]} ${onePart[2]}`;
  return m;
}

/** Aggregated token usage for one turn, shown on its final assistant row. */
export interface TurnUsage {
  /** The turn's last recorded `input_tokens` — the context size, not a sum. */
  inputTokens: number;
  /** Summed `output_tokens` across the turn's assistant records. */
  outputTokens: number;
}

/**
 * One usage entry per *turn*, keyed by the index of the turn's final assistant
 * row. A turn is a maximal run of adjacent assistant rows; anything else (a
 * user prompt, a meta injection) ends it. Claude Code writes each API step as
 * its own assistant record, so per-record usage rows repeat near-identical
 * `↑ ↓ · model · time` lines a dozen times a minute — this map is what lets
 * the list draw that line once, with honest turn totals.
 */
export function turnUsageByRow(rows: RawMessage[]): Map<number, TurnUsage> {
  const out = new Map<number, TurnUsage>();
  let sumOut = 0;
  let lastIn = 0;
  let sawUsage = false;
  for (let i = 0; i < rows.length; i++) {
    const msg = rows[i];
    if (msg.type !== "assistant") {
      sumOut = 0;
      lastIn = 0;
      sawUsage = false;
      continue;
    }
    const usage = msg.message?.usage;
    if (usage) {
      sumOut += usage.output_tokens;
      lastIn = usage.input_tokens;
      sawUsage = true;
    }
    const isTurnEnd = i === rows.length - 1 || rows[i + 1].type !== "assistant";
    if (isTurnEnd && sawUsage) {
      out.set(i, { inputTokens: lastIn, outputTokens: sumOut });
      sumOut = 0;
      lastIn = 0;
      sawUsage = false;
    }
  }
  return out;
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
