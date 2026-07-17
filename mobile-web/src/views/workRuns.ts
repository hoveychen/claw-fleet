// Work-run grouping for the mobile transcript — a port of the desktop's
// `workRuns.ts`, kept as a separate copy because mobile-web is a standalone
// package that shares no code with the desktop app. Unlike desktop it does not
// break runs at day boundaries: mobile-web has no day separators.

import type { RawMessage } from "../types";
import type { MetaRenderUnit } from "./metaGrouping";

/** Same tail-match rule as the desktop: the MCP tool name is namespaced by the
 *  server, so match `…fleet__ask` rather than the full id. codex's
 *  `request_user_input` is its decision tool. */
export function isDecisionTool(name: string): boolean {
  return name === "AskUserQuestion" || name === "request_user_input" || name.endsWith("fleet__ask");
}

/**
 * Rows that a work band folds — assistant records whose visible content is
 * pure scaffolding: thinking and tool calls, no prose. A record carrying any
 * non-empty text block is where the agent spoke; a decision tool is where the
 * conversation turned. Both stay full rows.
 */
export function isWorkRow(msg: RawMessage): boolean {
  if (msg.type !== "assistant" || !msg.message) return false;
  const content = msg.message.content;
  if (!Array.isArray(content) || content.length === 0) return false;
  let sawWork = false;
  for (const block of content) {
    if (block.type === "text") {
      if ((block.text ?? "").trim()) return false;
      continue;
    }
    if (block.type === "thinking" || block.type === "redacted_thinking") {
      sawWork = true;
      continue;
    }
    if (block.type === "tool_use") {
      const name = (block as { name?: string }).name ?? "";
      if (isDecisionTool(name)) return false;
      sawWork = true;
      continue;
    }
    return false;
  }
  return sawWork;
}

export type RenderUnit =
  | MetaRenderUnit
  | { kind: "work-group"; startLocal: number; msgs: RawMessage[] };

/** Second pass over `groupMetaRuns` output: collapse runs of ≥2 adjacent work
 *  rows into `work-group` units (a lone work row reads fine as rail steps). */
export function groupWorkRuns(units: MetaRenderUnit[]): RenderUnit[] {
  const out: RenderUnit[] = [];
  for (let k = 0; k < units.length; ) {
    const unit = units[k];
    if (unit.kind === "single" && isWorkRow(unit.msg)) {
      let j = k + 1;
      while (j < units.length) {
        const next = units[j];
        if (next.kind !== "single" || !isWorkRow(next.msg)) break;
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

/** Steps (tool calls + thinking segments) across a run, for the band label. */
export function countSteps(msgs: RawMessage[]): number {
  let steps = 0;
  for (const msg of msgs) {
    const content = msg.message?.content;
    if (!Array.isArray(content)) continue;
    for (const block of content) {
      if (
        block.type === "thinking" ||
        block.type === "redacted_thinking" ||
        block.type === "tool_use"
      ) {
        steps++;
      }
    }
  }
  return steps;
}

/** Longest band title before it gets clipped with an ellipsis. */
const TITLE_MAX = 64;

/** First sentence of a thinking text, clipped to `TITLE_MAX`. Sentence ends at
 *  CJK terminal punctuation, an English ". " (a bare dot may be a filename or
 *  version), or the first line break. */
export function firstSentence(text: string): string {
  const firstLine = text.trimStart().split("\n", 1)[0].trim();
  const m = firstLine.match(/^.*?(?:[。！？]|[.!?](?=\s|$))/);
  const sentence = (m ? m[0] : firstLine).trim();
  return sentence.length > TITLE_MAX ? `${sentence.slice(0, TITLE_MAX - 1)}…` : sentence;
}

/** Band title: the first sentence of the run's *last* thinking segment (with
 *  `--thinking-display summarized` those are model-written summaries). The
 *  first thinking often lands mid-run and describes only what comes next, so
 *  the last one — the run's most recent intent — reads as the band's
 *  conclusion. Null when the run has no usable thinking — the band falls back
 *  to a generic label. */
export function workRunTitle(msgs: RawMessage[]): string | null {
  for (let i = msgs.length - 1; i >= 0; i--) {
    const content = msgs[i].message?.content;
    if (!Array.isArray(content)) continue;
    for (let j = content.length - 1; j >= 0; j--) {
      const block = content[j];
      if (block.type !== "thinking") continue;
      const text = (block as { thinking?: unknown }).thinking;
      if (typeof text !== "string" || !text.trim()) continue;
      const sentence = firstSentence(text);
      if (sentence) return sentence;
    }
  }
  return null;
}
