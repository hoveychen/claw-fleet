import { useEffect, useLayoutEffect, useMemo, useRef, useState, memo } from "react";
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

const ContentBlocks = memo(function ContentBlocks({ content, resultMap, isPartial }: BlocksProps) {
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
});

// ── Single message row ────────────────────────────────────────────────────────

interface MsgProps {
  msg: RawMessage;
  resultMap: Map<string, ToolResultBlock>;
}

const MessageRow = memo(function MessageRow({ msg, resultMap }: MsgProps) {
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
});

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

const PAGE_SIZE = 100;

export function MessageList({ messages, isLoading }: Props) {
  const listRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  const topSentinelRef = useRef<HTMLDivElement>(null);
  const [visibleStart, setVisibleStart] = useState(0);
  // Saved before loading more; used by useLayoutEffect to restore scroll position
  const scrollAnchor = useRef<{ scrollTop: number; scrollHeight: number } | null>(null);

  const resultMap = useMemo(() => buildResultMap(messages), [messages]);

  const displayMsgs = useMemo(
    () => messages.filter((m) => m.type === "user" || m.type === "assistant"),
    [messages]
  );

  // Reset window when switching to a new session (total message count drops)
  const prevCountRef = useRef(displayMsgs.length);
  const sessionSwitchedRef = useRef(false);
  useEffect(() => {
    if (displayMsgs.length < prevCountRef.current) {
      setVisibleStart(0);
      sessionSwitchedRef.current = true;
    }
    prevCountRef.current = displayMsgs.length;
  }, [displayMsgs.length]);

  const effectiveStart = Math.max(visibleStart, displayMsgs.length - PAGE_SIZE);
  const visibleMsgs = displayMsgs.slice(effectiveStart);
  const hiddenCount = effectiveStart;

  // When effectiveStart advances due to new tail messages, save scroll anchor
  // so the layout effect below can compensate for the removed top element(s)
  const prevEffectiveStartRef = useRef(effectiveStart);
  if (effectiveStart > prevEffectiveStartRef.current && !scrollAnchor.current) {
    const scroller = listRef.current?.parentElement;
    if (scroller && listRef.current) {
      scrollAnchor.current = {
        scrollTop: scroller.scrollTop,
        scrollHeight: listRef.current.scrollHeight,
      };
    }
  }
  prevEffectiveStartRef.current = effectiveStart;

  // After prepending/trimming messages, restore scroll so the viewport doesn't jump
  useLayoutEffect(() => {
    const anchor = scrollAnchor.current;
    if (!anchor || !listRef.current) return;
    const scroller = listRef.current.parentElement;
    if (scroller) {
      scroller.scrollTop = anchor.scrollTop + (listRef.current.scrollHeight - anchor.scrollHeight);
    }
    scrollAnchor.current = null;
  });

  // Observe the top sentinel relative to the actual scroll container (.scroll_area)
  useEffect(() => {
    const sentinel = topSentinelRef.current;
    if (!sentinel || hiddenCount === 0) return;
    const scroller = listRef.current?.parentElement;
    if (!scroller) return;

    const observer = new IntersectionObserver(
      (entries) => {
        if (!entries[0].isIntersecting) return;
        // scrollAnchor acts as a loading guard: skip if a load is already in flight
        if (scrollAnchor.current) return;
        scrollAnchor.current = {
          scrollTop: scroller.scrollTop,
          scrollHeight: listRef.current!.scrollHeight,
        };
        setVisibleStart((prev) => Math.max(0, prev - PAGE_SIZE));
      },
      { root: scroller, threshold: 0 }
    );

    observer.observe(sentinel);
    return () => observer.disconnect();
  }, [hiddenCount]);

  // Auto-scroll to bottom when session switches or when user is already near the bottom
  useEffect(() => {
    const scroller = listRef.current?.parentElement;
    if (!scroller) return;
    if (sessionSwitchedRef.current) {
      sessionSwitchedRef.current = false;
      bottomRef.current?.scrollIntoView({ behavior: "instant" });
      return;
    }
    const distFromBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    if (distFromBottom < 200) {
      bottomRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [displayMsgs.length]);

  const lastAssistant = [...displayMsgs].reverse().find((m: RawMessage) => m.type === "assistant");
  const isWaiting = lastAssistant?.message?.stop_reason === "end_turn";

  if (isLoading) {
    return <div className={styles.loading}>Loading…</div>;
  }

  return (
    <div ref={listRef} className={styles.list}>
      {/* Top sentinel – triggers scroll-load when it enters the viewport */}
      <div ref={topSentinelRef} className={hiddenCount > 0 ? styles.sentinel : undefined} />
      {visibleMsgs.map((msg, i) => (
        <MessageRow key={msg.uuid ?? (effectiveStart + i)} msg={msg} resultMap={resultMap} />
      ))}
      {isWaiting && <WaitingIndicator />}
      <div ref={bottomRef} />
    </div>
  );
}
