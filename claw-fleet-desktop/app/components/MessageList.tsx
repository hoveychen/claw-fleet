import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, memo } from "react";
import { useTranslation } from "react-i18next";
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
import { CompactSummaryBlock } from "./blocks/CompactSummaryBlock";
import styles from "./MessageList.module.css";

// ── Search highlight ─────────────────────────────────────────────────────────

function HighlightedText({ text, terms }: { text: string; terms: string[] }) {
  if (!terms.length) return <>{text}</>;
  // Build a regex matching any of the search terms (case-insensitive)
  const escaped = terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const regex = new RegExp(`(${escaped.join("|")})`, "gi");
  const parts = text.split(regex);
  return (
    <>
      {parts.map((part, i) =>
        regex.test(part) ? (
          <mark key={i} style={{ background: "#fbbf24", color: "#000", borderRadius: "2px" }}>
            {part}
          </mark>
        ) : (
          <span key={i}>{part}</span>
        )
      )}
    </>
  );
}

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

// ── Write baseline replay ─────────────────────────────────────────────────────

/** Strip Claude Code's `cat -n` line-number prefix from a Read result. */
function stripCatNFormat(s: string): string {
  return s.split("\n").map((line) => line.replace(/^\s*\d+\t/, "")).join("\n");
}

/**
 * Walk session messages forward, tracking each `file_path`'s reconstructed
 * content as Read results land and Write/Edit/MultiEdit ops apply. For each
 * `Write` tool_use we capture the prior content (if known) so the diff view
 * can render an accurate "before vs after". Edit/MultiEdit don't need this —
 * their input already carries old/new strings.
 */
function buildWriteBaselineMap(messages: RawMessage[]): Map<string, string> {
  const fileState = new Map<string, string>();
  const baseline = new Map<string, string>();
  const pendingReads = new Map<string, { path: string; full: boolean }>();

  for (const msg of messages) {
    if (!msg.message) continue;
    const content = msg.message.content;
    if (!Array.isArray(content)) continue;

    for (const block of content) {
      if (block.type === "tool_use") {
        const tu = block as ToolUseBlock;
        const path = typeof tu.input.file_path === "string" ? tu.input.file_path : undefined;
        if (!path) continue;

        if (tu.name === "Read") {
          const full =
            tu.input.offset === undefined && tu.input.limit === undefined;
          pendingReads.set(tu.id, { path, full });
        } else if (tu.name === "Write") {
          const cur = fileState.get(path);
          if (cur !== undefined) baseline.set(tu.id, cur);
          const next = typeof tu.input.content === "string" ? tu.input.content : "";
          fileState.set(path, next);
        } else if (tu.name === "Edit") {
          const cur = fileState.get(path);
          if (cur !== undefined) {
            const oldS = String(tu.input.old_string ?? "");
            const newS = String(tu.input.new_string ?? "");
            const replaceAll = tu.input.replace_all === true;
            const next = replaceAll ? cur.split(oldS).join(newS) : cur.replace(oldS, newS);
            fileState.set(path, next);
          }
        } else if (tu.name === "MultiEdit") {
          let cur = fileState.get(path);
          if (cur !== undefined) {
            const edits = Array.isArray(tu.input.edits)
              ? (tu.input.edits as Array<{
                  old_string?: string;
                  new_string?: string;
                  replace_all?: boolean;
                }>)
              : [];
            for (const e of edits) {
              const oldS = String(e.old_string ?? "");
              const newS = String(e.new_string ?? "");
              cur = e.replace_all
                ? cur.split(oldS).join(newS)
                : cur.replace(oldS, newS);
            }
            fileState.set(path, cur);
          }
        }
      } else if (block.type === "tool_result") {
        const tr = block as ToolResultBlock;
        const pending = pendingReads.get(tr.tool_use_id);
        if (
          pending &&
          pending.full &&
          !tr.is_error &&
          typeof tr.content === "string"
        ) {
          fileState.set(pending.path, stripCatNFormat(tr.content));
        }
        pendingReads.delete(tr.tool_use_id);
      }
    }
  }

  return baseline;
}

// ── Content blocks renderer ───────────────────────────────────────────────────

interface BlocksProps {
  content: ContentBlock[];
  resultMap: Map<string, ToolResultBlock>;
  baselineMap: Map<string, string>;
  isPartial: boolean;
  searchTerms?: string[] | null;
}

const ContentBlocks = memo(function ContentBlocks({ content, resultMap, baselineMap, isPartial, searchTerms }: BlocksProps) {
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
          searchTerms={searchTerms}
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
          baseline={baselineMap.has(toolBlock.id) ? baselineMap.get(toolBlock.id) : null}
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
  baselineMap: Map<string, string>;
  searchTerms?: string[] | null;
  msgIdx?: number;
}

const MessageRow = memo(function MessageRow({ msg, resultMap, baselineMap, searchTerms, msgIdx }: MsgProps) {
  if (!msg.message) return null;

  const isAssistant = msg.type === "assistant";
  const isUser = msg.type === "user";
  const content = msg.message.content;

  // Context-compaction marker: Claude Code injects a synthetic user message
  // tagged `isCompactSummary: true` at the boundary. Render it as a compact
  // banner instead of the giant blob.
  if (isUser && msg.isCompactSummary) {
    let summaryText = "";
    if (typeof content === "string") {
      summaryText = content;
    } else if (Array.isArray(content)) {
      for (const b of content) {
        if (b.type === "text") summaryText += (b as { type: "text"; text: string }).text;
      }
    }
    return (
      <div className={styles.compact_row} data-msg-idx={msgIdx}>
        <CompactSummaryBlock summary={summaryText} />
      </div>
    );
  }

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
      data-msg-idx={msgIdx}
    >
      {isAssistant && <span className={`${styles.dot} ${dotClass}`} />}
      <div className={styles.content}>
        {isAssistant && Array.isArray(content) && (
          <ContentBlocks
            content={content}
            resultMap={resultMap}
            baselineMap={baselineMap}
            isPartial={isPartial}
            searchTerms={searchTerms}
          />
        )}
        {isUser && (
          <div className={styles.user_text}>
            {typeof content === "string"
              ? (searchTerms ? <HighlightedText text={content} terms={searchTerms} /> : content)
              : Array.isArray(content)
                ? content
                    .filter((b) => b.type !== "tool_result")
                    .map((b, i) =>
                      b.type === "text" ? (
                        <span key={i}>
                          {searchTerms
                            ? <HighlightedText text={(b as { type: "text"; text: string }).text} terms={searchTerms} />
                            : (b as { type: "text"; text: string }).text}
                        </span>
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
  searchQuery?: string | null;
}

const PAGE_SIZE = 100;

/** Extract plain text from a message for search matching. */
function messageText(msg: RawMessage): string {
  if (!msg.message) return "";
  const content = msg.message.content;
  if (typeof content === "string") return content;
  if (!Array.isArray(content)) return "";
  return content
    .map((b) => {
      if (b.type === "text") return (b as { type: "text"; text: string }).text;
      if (b.type === "thinking") return (b as { type: "thinking"; thinking: string }).thinking;
      return "";
    })
    .join(" ");
}

export function MessageList({ messages, isLoading, searchQuery }: Props) {
  const { t } = useTranslation();
  const listRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  // visibleStart tracks the actual start index into displayMsgs.
  // -1 is a sentinel meaning "show the tail (last PAGE_SIZE)".
  const [visibleStart, setVisibleStart] = useState(-1);
  // Saved before loading more; used by useLayoutEffect to restore scroll position
  const scrollAnchor = useRef<{ scrollTop: number; scrollHeight: number } | null>(null);

  const resultMap = useMemo(() => buildResultMap(messages), [messages]);
  const baselineMap = useMemo(() => buildWriteBaselineMap(messages), [messages]);

  const displayMsgs = useMemo(
    () => messages.filter((m) => m.type === "user" || m.type === "assistant"),
    [messages]
  );

  // Parse search terms once for matching and highlighting
  const searchTerms = useMemo(() => {
    if (!searchQuery || searchQuery.trim().length < 2) return null;
    return searchQuery.trim().toLowerCase().split(/\s+/).filter(Boolean);
  }, [searchQuery]);

  // Find the index of the first matching message for search navigation
  const searchMatchIndex = useMemo(() => {
    if (!searchTerms || displayMsgs.length === 0) return -1;
    for (let i = 0; i < displayMsgs.length; i++) {
      const text = messageText(displayMsgs[i]).toLowerCase();
      if (searchTerms.every((term) => text.includes(term))) return i;
    }
    return -1;
  }, [searchTerms, displayMsgs]);

  // Reset window when switching to a new session (total message count drops)
  const prevCountRef = useRef(displayMsgs.length);
  const sessionSwitchedRef = useRef(false);
  const searchScrolledRef = useRef(false);
  useEffect(() => {
    if (
      displayMsgs.length < prevCountRef.current ||
      (prevCountRef.current === 0 && displayMsgs.length > 0)
    ) {
      sessionSwitchedRef.current = true;
      searchScrolledRef.current = false;

      // If we have a search match, start the view window around that match
      if (searchMatchIndex >= 0) {
        const start = Math.max(0, searchMatchIndex - 10); // show some context before match
        setVisibleStart(start);
      } else {
        setVisibleStart(-1);
      }
    }
    prevCountRef.current = displayMsgs.length;
  }, [displayMsgs.length, searchMatchIndex]);

  // Compute effective start: -1 means "tail mode" (follow latest messages)
  const tailStart = Math.max(0, displayMsgs.length - PAGE_SIZE);
  const effectiveStart = visibleStart === -1 ? tailStart : Math.min(visibleStart, tailStart);
  const visibleMsgs = displayMsgs.slice(effectiveStart);
  const hiddenCount = effectiveStart;

  // When in tail mode and new messages arrive, the window auto-advances.
  // When user has scrolled up (visibleStart >= 0), the window stays put and grows.
  const prevEffectiveStartRef = useRef(effectiveStart);
  if (
    visibleStart === -1 &&
    effectiveStart > prevEffectiveStartRef.current &&
    !scrollAnchor.current
  ) {
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

  const loadMore = useCallback(() => {
    if (hiddenCount === 0 || scrollAnchor.current) return;
    const scroller = listRef.current?.parentElement;
    if (scroller && listRef.current) {
      scrollAnchor.current = {
        scrollTop: scroller.scrollTop,
        scrollHeight: listRef.current.scrollHeight,
      };
    }
    setVisibleStart(Math.max(0, prevEffectiveStartRef.current - PAGE_SIZE));
  }, [hiddenCount]);

  // Auto-scroll to bottom (or to search match) when session switches
  useEffect(() => {
    const scroller = listRef.current?.parentElement;
    if (!scroller) return;
    if (sessionSwitchedRef.current) {
      sessionSwitchedRef.current = false;
      // If we have a search match, scroll to it instead of the bottom
      if (searchMatchIndex >= 0 && !searchScrolledRef.current) {
        searchScrolledRef.current = true;
        // Find the DOM element for the matching message
        const matchRelIdx = searchMatchIndex - effectiveStart;
        if (matchRelIdx >= 0 && listRef.current) {
          const rows = listRef.current.querySelectorAll("[data-msg-idx]");
          for (const row of rows) {
            if (Number(row.getAttribute("data-msg-idx")) === searchMatchIndex) {
              row.scrollIntoView({ behavior: "instant", block: "center" });
              return;
            }
          }
        }
      }
      bottomRef.current?.scrollIntoView({ behavior: "instant" });
      return;
    }
    const distFromBottom = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight;
    if (distFromBottom < 200) {
      bottomRef.current?.scrollIntoView({ behavior: "smooth" });
    }
  }, [displayMsgs.length, searchMatchIndex, effectiveStart]);

  const lastAssistant = [...displayMsgs].reverse().find((m: RawMessage) => m.type === "assistant");
  const isWaiting = lastAssistant?.message?.stop_reason === "end_turn";

  if (isLoading) {
    return <div className={styles.loading}>Loading…</div>;
  }

  return (
    <div ref={listRef} className={styles.list}>
      {hiddenCount > 0 && (
        <button className={styles.load_more} onClick={loadMore}>
          ↑ {t("detail.load_more", { count: Math.min(PAGE_SIZE, hiddenCount) })}
        </button>
      )}
      {visibleMsgs.map((msg, i) => (
        <MessageRow
          key={msg.uuid ?? (effectiveStart + i)}
          msg={msg}
          resultMap={resultMap}
          baselineMap={baselineMap}
          searchTerms={searchTerms}
          msgIdx={effectiveStart + i}
        />
      ))}
      {isWaiting && <WaitingIndicator />}
      <div ref={bottomRef} />
    </div>
  );
}
