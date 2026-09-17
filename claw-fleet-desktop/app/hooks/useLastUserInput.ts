import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useSessionsStore } from "../store";
import type {
  ContentBlock,
  RawMessage,
  TextBlock,
  ToolResultBlock,
  ToolUseBlock,
} from "../types";

/** One answered question from the previous card: the question's short label
 *  (its first line, clipped) and the value the user picked or typed. */
export interface AnswerEntry {
  label: string;
  value: string;
}

/**
 * The last thing the *user* said before this question — either a prompt they
 * typed, or the answer they gave to the previous decision card. Shown above a
 * question card so the round's starting point stays visible while answering.
 */
export interface LastUserInput {
  /** `prompt` = typed message, `answer` = answer to a previous ask-family card. */
  kind: "prompt" | "answer";
  /** Flat text — the prompt itself, or the answer values joined. */
  text: string;
  /** Per-question breakdown; only set when `kind === "answer"`. */
  answers?: AnswerEntry[];
}

export interface LastUserInputResult {
  input: LastUserInput | null;
  loading: boolean;
}

// How far back to read the transcript. The user's last input is almost always a
// handful of messages from the tail, so a few hundred lines is plenty even for
// a chatty agent.
const TRANSCRIPT_TAIL = 400;
// Hard cap on the rendered text so a pasted wall of text can't blow up the card.
const MAX_CHARS = 4000;
// Question labels are clipped to one short line — an ask-family answer key is
// the question's whole body, which is often an entire report.
const MAX_LABEL_CHARS = 48;

// Tool names whose *result* counts as the user having "spoken" — answering an
// AskUserQuestion card or approving a plan is user input, not agent plumbing.
// A regular Bash/Read tool_result is NOT user input.
const ASK_TOOL_NAMES = new Set([
  "AskUserQuestion",
  "ExitPlanMode",
  "mcp__fleet__fleet__ask",
  "mcp__fleet__fleet__render_a2ui",
]);

function isAskTool(name: string): boolean {
  return ASK_TOOL_NAMES.has(name) || name.includes("fleet__ask");
}

function asArray(content: ContentBlock[] | string | undefined): ContentBlock[] {
  return Array.isArray(content) ? content : [];
}

/**
 * Strip the machinery Claude Code wraps around a typed prompt — `<system-reminder>`
 * blocks (hook context, memory recalls) and `<command-name>`/`<command-message>`
 * envelopes — leaving what the user actually typed.
 */
export function stripPromptEnvelope(text: string): string {
  return text
    .replace(/<system-reminder>[\s\S]*?<\/system-reminder>/g, "")
    .replace(/<command-(name|message|args)>[\s\S]*?<\/command-\1>/g, "")
    .replace(/<local-command-std(out|err)>[\s\S]*?<\/local-command-std\1>/g, "")
    .trim();
}

/** Flatten a tool_result's content, which is a string on the wire for built-in
 *  tools and a block array for MCP tools (fleet__ask). */
function toolResultText(block: ToolResultBlock): string {
  const content = block.content as ContentBlock[] | string | undefined;
  if (typeof content === "string") return content.trim();
  return asArray(content)
    .filter((b): b is TextBlock => b.type === "text")
    .map((b) => b.text)
    .join("\n")
    .trim();
}

/**
 * Shorten an answer key into a one-line question label. `fleet__ask` keys are
 * the full question body — a TTS summary line, a `---` separator, then the
 * whole report — so the first line before the separator is the closest thing
 * to a title. Form-field answers key on the field name, which is already short.
 */
export function answerLabel(key: string): string {
  const head = key.split(/\n---\n/)[0].split("\n")[0];
  const plain = head
    .replace(/[*`#>]/g, "")
    .replace(/\s+/g, " ")
    .trim();
  return plain.length > MAX_LABEL_CHARS
    ? `${plain.slice(0, MAX_LABEL_CHARS)}…`
    : plain;
}

/**
 * Render a card answer for display. `fleet__ask` / `AskUserQuestion` return
 * `{"answers": {<question or field name>: <value>}}`; the keys are the full
 * question bodies (often a whole report), so each one is clipped to a short
 * label and paired with its value. Anything else — `TASK FINISHED`, a plan
 * approval, an unknown shape — falls back to one unlabelled entry.
 */
export function formatAnswer(raw: string): AnswerEntry[] {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (parsed && typeof parsed === "object") {
      const answers = (parsed as { answers?: unknown }).answers;
      if (answers && typeof answers === "object") {
        const out: AnswerEntry[] = [];
        for (const [key, v] of Object.entries(answers as Record<string, unknown>)) {
          const value = (typeof v === "string" ? v : JSON.stringify(v)).trim();
          if (!value) continue;
          const label = answerLabel(key);
          out.push({ label: label === value ? "" : label, value });
        }
        if (out.length) return out;
      }
    }
  } catch {
    // Not JSON — fall through to the raw text.
  }
  return raw ? [{ label: "", value: raw }] : [];
}

// The collapsed bar shows one line of the last input. CSS ellipsis handles the
// visual clip; this cap only stops a megabyte of text reaching the DOM as a
// single unbreakable node.
const MAX_SNIPPET_CHARS = 200;

/**
 * Flatten one input value into a single line for the collapsed bar: markdown
 * markup, fenced code bodies and image syntax are stripped (a `![](…)` data URI
 * would otherwise become the whole snippet) and every run of whitespace becomes
 * one space, so a multi-line answer can never grow the bar past one row.
 */
export function oneLineSnippet(text: string, max = MAX_SNIPPET_CHARS): string {
  const plain = text
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/!\[[^\]]*\]\([^)]*\)/g, " ")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/<[^>]*>/g, " ")
    .replace(/[*_`#>|]/g, "")
    .replace(/\s+/g, " ")
    .trim();
  return plain.length > max ? `${plain.slice(0, max)}…` : plain;
}

// A short one-liner costs almost nothing to show and saves a click, so the bar
// opens itself for it. Anything longer — or more than one answer — stays
// collapsed, which is the whole point of the bar.
const AUTO_EXPAND_CHARS = 120;

/**
 * Should the collapsed bar open itself? Only for a single entry whose one-line
 * form is short enough that showing it cannot push the question off-screen.
 */
export function shouldAutoExpand(entries: AnswerEntry[]): boolean {
  if (entries.length !== 1) return false;
  return oneLineSnippet(entries[0].value, AUTO_EXPAND_CHARS + 1).length <= AUTO_EXPAND_CHARS;
}

/**
 * Walk backwards to the user's last real input and render it: a typed prompt,
 * or their answer to the previous ask-family card. Returns null when the
 * transcript holds neither (e.g. a session's very first question).
 */
export function findLastUserInput(messages: RawMessage[]): LastUserInput | null {
  const toolNameById = new Map<string, string>();
  for (const m of messages) {
    for (const b of asArray(m.message?.content)) {
      if (b.type === "tool_use") {
        const tu = b as ToolUseBlock;
        toolNameById.set(tu.id, tu.name);
      }
    }
  }

  for (let i = messages.length - 1; i >= 0; i--) {
    const msg = messages[i];
    if (msg.type !== "user" || !msg.message) continue;
    const content = msg.message.content;

    if (typeof content === "string") {
      const text = stripPromptEnvelope(content);
      if (text) return { kind: "prompt", text: clamp(text) };
      continue;
    }

    const blocks = asArray(content);
    const typed = blocks
      .filter((b): b is TextBlock => b.type === "text")
      .map((b) => stripPromptEnvelope(b.text))
      .filter(Boolean)
      .join("\n")
      .trim();
    if (typed) return { kind: "prompt", text: clamp(typed) };

    for (const b of blocks) {
      if (b.type !== "tool_result") continue;
      const tr = b as ToolResultBlock;
      // A failed ask call was never actually answered — keep looking further back.
      if (tr.is_error) continue;
      const name = toolNameById.get(tr.tool_use_id);
      if (!name || !isAskTool(name)) continue;
      const answers = formatAnswer(toolResultText(tr)).map((a) => ({
        label: a.label,
        value: clamp(a.value),
      }));
      if (answers.length) {
        return {
          kind: "answer",
          text: answers.map((a) => a.value).join("\n"),
          answers,
        };
      }
    }
  }
  return null;
}

function clamp(text: string): string {
  return text.length > MAX_CHARS ? `${text.slice(0, MAX_CHARS)}…` : text;
}

/**
 * Loads the transcript for `sessionId` and returns the user's last input before
 * this question. `requestId` retriggers the fetch when a fresh decision arrives
 * for the same session.
 */
export function useLastUserInput(
  sessionId: string,
  requestId: string,
): LastUserInputResult {
  const jsonlPath = useSessionsStore(
    (s) => s.sessions.find((sess) => sess.id === sessionId)?.jsonlPath,
  );
  const [input, setInput] = useState<LastUserInput | null>(null);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!jsonlPath) {
      setInput(null);
      setLoading(false);
      return;
    }
    let cancelled = false;
    setLoading(true);
    invoke<RawMessage[]>("get_messages_tail", {
      jsonlPath,
      tail: TRANSCRIPT_TAIL,
    })
      .then((raw) => {
        if (cancelled) return;
        setInput(findLastUserInput(raw));
      })
      .catch(() => {
        if (!cancelled) setInput(null);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [jsonlPath, requestId]);

  return { input, loading };
}
