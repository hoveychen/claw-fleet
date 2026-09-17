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

/**
 * The last thing the *user* said before this question — either a prompt they
 * typed, or the answer they gave to the previous decision card. Shown above a
 * question card so the round's starting point stays visible while answering.
 */
export interface LastUserInput {
  /** `prompt` = typed message, `answer` = answer to a previous ask-family card. */
  kind: "prompt" | "answer";
  text: string;
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
 * Render a card answer for display. `fleet__ask` / `AskUserQuestion` return
 * `{"answers": {<question or field name>: <value>}}`; the keys are the full
 * question bodies (often a whole report), so only the values are shown.
 * Anything else — `TASK FINISHED`, a plan approval, an unknown shape — falls
 * back to the raw text.
 */
export function formatAnswer(raw: string): string {
  try {
    const parsed = JSON.parse(raw) as unknown;
    if (parsed && typeof parsed === "object") {
      const answers = (parsed as { answers?: unknown }).answers;
      if (answers && typeof answers === "object") {
        const vals = Object.values(answers as Record<string, unknown>)
          .map((v) => (typeof v === "string" ? v : JSON.stringify(v)))
          .map((v) => v.trim())
          .filter(Boolean);
        if (vals.length) return vals.join("\n");
      }
    }
  } catch {
    // Not JSON — fall through to the raw text.
  }
  return raw;
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
      const text = formatAnswer(toolResultText(tr)).trim();
      if (text) return { kind: "answer", text: clamp(text) };
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
