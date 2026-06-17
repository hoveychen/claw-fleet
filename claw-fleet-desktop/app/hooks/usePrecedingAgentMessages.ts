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
 * One chunk of agent narration that occurred between the user's last input and
 * the current question — i.e. an assistant message's plain `text` content, with
 * tool_use / tool_result / thinking blocks stripped out.
 */
export interface PrecedingAgentMessage {
  uuid?: string;
  timestamp?: string;
  text: string;
}

export interface PrecedingAgentMessagesResult {
  messages: PrecedingAgentMessage[];
  loading: boolean;
}

// How far back to read the transcript. The window we actually care about (since
// the user's last input) is almost always a handful of messages at the tail, so
// a few hundred lines is plenty even for a chatty agent.
const TRANSCRIPT_TAIL = 400;
// Hard cap on how many narration chunks we surface, so a pathological run that
// never paused for user input can't blow up the card.
const MAX_CHUNKS = 40;

// Tool names whose *result* counts as the user having "spoken" — answering an
// AskUserQuestion card or approving a plan is user input, not agent plumbing.
// A regular Bash/Read tool_result is NOT user input and must not end the span.
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

/** Concatenated plain text of an assistant message (skips non-text blocks). */
function assistantText(msg: RawMessage): string {
  if (msg.type !== "assistant" || !msg.message) return "";
  const content = msg.message.content;
  if (typeof content === "string") return content.trim();
  return asArray(content)
    .filter((b): b is TextBlock => b.type === "text")
    .map((b) => b.text)
    .join("\n")
    .trim();
}

/**
 * True when this user-role message represents the user actually saying
 * something: a typed prompt (string or text block), or a tool_result answering
 * an ask-family tool. Regular tool_results (Bash, Read, …) return false so the
 * agent's working span isn't cut short by its own tool plumbing.
 */
function isUserInput(
  msg: RawMessage,
  toolNameById: Map<string, string>,
): boolean {
  if (msg.type !== "user" || !msg.message) return false;
  const content = msg.message.content;
  if (typeof content === "string") return content.trim().length > 0;
  const blocks = asArray(content);
  if (blocks.some((b) => b.type === "text" && (b as TextBlock).text.trim())) {
    return true;
  }
  for (const b of blocks) {
    if (b.type === "tool_result") {
      const name = toolNameById.get((b as ToolResultBlock).tool_use_id);
      if (name && isAskTool(name)) return true;
    }
  }
  return false;
}

/**
 * Extract the agent's narration since the user's last input. The current
 * question is always the latest thing the agent did, so everything the agent
 * said after the last user input — across any number of intervening tool
 * calls — is the lead-up to this question.
 */
export function slicePrecedingMessages(
  messages: RawMessage[],
): PrecedingAgentMessage[] {
  // Map every tool_use id -> name so we can classify tool_results later.
  const toolNameById = new Map<string, string>();
  for (const m of messages) {
    for (const b of asArray(m.message?.content)) {
      if (b.type === "tool_use") {
        const tu = b as ToolUseBlock;
        toolNameById.set(tu.id, tu.name);
      }
    }
  }

  // Find the last genuine user input; the span is everything after it.
  let boundary = -1;
  for (let i = messages.length - 1; i >= 0; i--) {
    if (isUserInput(messages[i], toolNameById)) {
      boundary = i;
      break;
    }
  }

  const out: PrecedingAgentMessage[] = [];
  for (const m of messages.slice(boundary + 1)) {
    const text = assistantText(m);
    if (text) out.push({ uuid: m.uuid, timestamp: m.timestamp, text });
  }
  return out.length > MAX_CHUNKS ? out.slice(out.length - MAX_CHUNKS) : out;
}

/**
 * Loads the transcript for `sessionId` and returns the agent's plain-text
 * narration since the user's last input — the words an agent often buries in
 * the turn leading up to a question. `requestId` retriggers the fetch when a
 * fresh decision arrives for the same session.
 */
export function usePrecedingAgentMessages(
  sessionId: string,
  requestId: string,
): PrecedingAgentMessagesResult {
  const jsonlPath = useSessionsStore(
    (s) => s.sessions.find((sess) => sess.id === sessionId)?.jsonlPath,
  );
  const [messages, setMessages] = useState<PrecedingAgentMessage[]>([]);
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (!jsonlPath) {
      setMessages([]);
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
        setMessages(slicePrecedingMessages(raw));
      })
      .catch(() => {
        if (!cancelled) setMessages([]);
      })
      .finally(() => {
        if (!cancelled) setLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [jsonlPath, requestId]);

  return { messages, loading };
}
