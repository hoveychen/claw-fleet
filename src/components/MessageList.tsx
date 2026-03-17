import { useEffect, useRef } from "react";
import type {
  ContentBlock,
  RawMessage,
  ToolResultBlock,
  ToolUseBlock,
} from "../types";
import { TextBlock } from "./blocks/TextBlock";
import { ThinkingBlock } from "./blocks/ThinkingBlock";
import {
  GroupedToolUseBlocks,
  ToolUseBlock as ToolUseBlockComp,
} from "./blocks/ToolUseBlock";
import styles from "./MessageList.module.css";

// ── Tool result lookup ────────────────────────────────────────────────────────

function buildResultMap(
  messages: RawMessage[]
): Map<string, ToolResultBlock> {
  const map = new Map<string, ToolResultBlock>();
  for (const msg of messages) {
    if (msg.type !== "user" || !msg.message) continue;
    const content = msg.message.content;
    if (!Array.isArray(content)) continue;
    for (const block of content) {
      if (block.type === "tool_result") {
        const r = block as ToolResultBlock;
        map.set(r.tool_use_id, r);
      }
    }
  }
  return map;
}

// ── Content blocks renderer ───────────────────────────────────────────────────

interface BlocksProps {
  content: ContentBlock[];
  resultMap: Map<string, ToolResultBlock>;
  isPartial: boolean;
}

function ContentBlocks({ content, resultMap, isPartial }: BlocksProps) {
  const elements: React.ReactNode[] = [];
  let i = 0;

  while (i < content.length) {
    const block = content[i];

    if (block.type === "text") {
      elements.push(
        <TextBlock
          key={i}
          text={(block as { type: "text"; text: string }).text}
          isPartial={isPartial && i === content.length - 1}
        />
      );
      i++;
      continue;
    }

    if (block.type === "thinking") {
      elements.push(
        <ThinkingBlock
          key={i}
          thinking={(block as { type: "thinking"; thinking: string }).thinking}
        />
      );
      i++;
      continue;
    }

    if (block.type === "redacted_thinking") {
      elements.push(
        <div key={i} className={styles.redacted}>
          [Redacted thinking]
        </div>
      );
      i++;
      continue;
    }

    if (block.type === "tool_use") {
      const toolBlock = block as ToolUseBlock;
      const result = resultMap.get(toolBlock.id);

      // Check if the next blocks are also read-only tools → group them
      const READ_ONLY = new Set([
        "Read", "Grep", "Glob", "WebSearch", "WebFetch", "TodoWrite", "TodoRead",
      ]);

      if (READ_ONLY.has(toolBlock.name)) {
        const group: Array<{ block: ToolUseBlock; result?: ToolResultBlock }> =
          [{ block: toolBlock, result }];

        let j = i + 1;
        while (j < content.length && content[j].type === "tool_use") {
          const next = content[j] as ToolUseBlock;
          if (!READ_ONLY.has(next.name)) break;
          group.push({ block: next, result: resultMap.get(next.id) });
          j++;
        }

        if (group.length >= 2) {
          elements.push(<GroupedToolUseBlocks key={i} blocks={group} />);
          i = j;
          continue;
        }
      }

      elements.push(
        <ToolUseBlockComp
          key={i}
          block={toolBlock}
          result={result}
          isPartial={isPartial && !result}
        />
      );
      i++;
      continue;
    }

    // Unknown block type: skip
    i++;
  }

  return <>{elements}</>;
}

// ── Single message row ────────────────────────────────────────────────────────

interface MsgProps {
  msg: RawMessage;
  resultMap: Map<string, ToolResultBlock>;
}

function MessageRow({ msg, resultMap }: MsgProps) {
  if (!msg.message) return null;

  const isAssistant = msg.type === "assistant";
  const isUser = msg.type === "user";
  const content = msg.message.content;

  // User messages: skip pure tool-result messages (rendered inline in tool blocks)
  if (isUser) {
    if (Array.isArray(content)) {
      const hasText = content.some((b) => b.type !== "tool_result");
      if (!hasText) return null;
    }
  }

  const isPartial =
    isAssistant && msg.message.stop_reason === null;

  // Status dot
  const stopReason = msg.message.stop_reason;
  const dotClass =
    stopReason === "end_turn"
      ? styles.dot_success
      : stopReason === "tool_use"
        ? isPartial
          ? styles.dot_progress
          : styles.dot_success
        : isPartial
          ? styles.dot_progress
          : "";

  return (
    <div
      className={`${styles.message} ${isAssistant ? styles.assistant : styles.user}`}
    >
      {isAssistant && <span className={`${styles.dot} ${dotClass}`} />}
      <div className={styles.content}>
        {isAssistant && Array.isArray(content) && (
          <ContentBlocks
            content={content}
            resultMap={resultMap}
            isPartial={isPartial}
          />
        )}
        {isUser && (
          <div className={styles.user_text}>
            {typeof content === "string"
              ? content
              : Array.isArray(content)
                ? content
                    .filter((b) => b.type !== "tool_result")
                    .map((b, i) =>
                      b.type === "text" ? (
                        <span key={i}>{(b as { type: "text"; text: string }).text}</span>
                      ) : null
                    )
                : null}
          </div>
        )}
        {isAssistant && msg.message.usage && (
          <div className={styles.usage}>
            ↑{msg.message.usage.input_tokens} ↓{msg.message.usage.output_tokens}
            {msg.message.model && (
              <span className={styles.model}> · {msg.message.model}</span>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

// ── Waiting for input indicator ───────────────────────────────────────────────

function WaitingIndicator() {
  return (
    <div className={styles.waiting}>
      <span className={styles.waiting_dot} />
      Waiting for input
    </div>
  );
}

// ── MessageList ───────────────────────────────────────────────────────────────

interface Props {
  messages: RawMessage[];
  isLoading: boolean;
}

export function MessageList({ messages, isLoading }: Props) {
  const bottomRef = useRef<HTMLDivElement>(null);

  // Auto-scroll to bottom on new messages
  useEffect(() => {
    bottomRef.current?.scrollIntoView({ behavior: "smooth" });
  }, [messages.length]);

  const resultMap = buildResultMap(messages);

  // Only show user/assistant messages
  const displayMsgs = messages.filter(
    (m) => m.type === "user" || m.type === "assistant"
  );

  // Check if last assistant message is waiting for input
  const lastAssistant = [...displayMsgs]
    .reverse()
    .find((m) => m.type === "assistant");
  const isWaiting = lastAssistant?.message?.stop_reason === "end_turn";

  if (isLoading) {
    return <div className={styles.loading}>Loading…</div>;
  }

  return (
    <div className={styles.list}>
      {displayMsgs.map((msg, i) => (
        <MessageRow key={msg.uuid ?? i} msg={msg} resultMap={resultMap} />
      ))}
      {isWaiting && <WaitingIndicator />}
      <div ref={bottomRef} />
    </div>
  );
}
