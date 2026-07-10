import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, memo } from "react";
import { useTranslation } from "react-i18next";
import type {
  ContentBlock,
  DecisionHistoryRecord,
  RawMessage,
  ToolResultBlock,
  ToolUseBlock,
} from "../types";
import { buildToolResultMetaMap, isDecisionTool } from "../toolResults";
import { nextVisibleCount, visibleCountForMatch, windowSlice } from "../messageWindow";
import { TextBlock } from "./blocks/TextBlock";
import { ThinkingBlock } from "./blocks/ThinkingBlock";
import {
  GroupedToolUseBlocks,
  ToolUseBlock as ToolUseBlockComp,
} from "./blocks/ToolUseBlock";
import { DecisionToolCard, hasDecisionQuestions } from "./blocks/DecisionToolCard";
import { UserContent } from "./blocks/UserContent";
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

// ── Content blocks renderer ───────────────────────────────────────────────────

interface BlocksProps {
  content: ContentBlock[];
  resultMap: Map<string, ToolResultBlock>;
  /** tool_use_id → Claude Code's structured `toolUseResult` for that call. */
  metaMap: Map<string, unknown>;
  /** Session decision records; supply asset ids for image-bearing cards. */
  decisionRecords: DecisionHistoryRecord[];
  isPartial: boolean;
  searchTerms?: string[] | null;
}

const ContentBlocks = memo(function ContentBlocks({ content, resultMap, metaMap, decisionRecords, isPartial, searchTerms }: BlocksProps) {
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

      // A decision card is where the conversation turned — never let it degrade
      // into the generic card's `JSON.stringify(input)` header. An unrenderable
      // shape (rejected call, future schema) still falls through to that card.
      if (isDecisionTool(toolBlock.name) && hasDecisionQuestions(toolBlock.input)) {
        elements.push(
          <DecisionToolCard
            key={i}
            block={toolBlock}
            result={result}
            meta={metaMap.get(toolBlock.id)}
            records={decisionRecords}
            isPartial={isPartial && !result}
          />
        );
        i++;
        continue;
      }

      // Note: the Claude Code Workflow tool renders as an ordinary tool card
      // here; its progress DAG is surfaced at the session level (the "Workflow"
      // tab in SessionDetail), not inline in the conversation.

      // Check if the next blocks are also read-only tools → group them
      const READ_ONLY = new Set([
        "Read", "Grep", "Glob", "WebSearch", "WebFetch", "TodoWrite", "TodoRead",
      ]);

      if (READ_ONLY.has(toolBlock.name)) {
        const group: Array<{ block: ToolUseBlock; result?: ToolResultBlock; meta?: unknown }> =
          [{ block: toolBlock, result, meta: metaMap.get(toolBlock.id) }];

        let j = i + 1;
        while (j < content.length && content[j].type === "tool_use") {
          const next = content[j] as ToolUseBlock;
          if (!READ_ONLY.has(next.name)) break;
          group.push({
            block: next,
            result: resultMap.get(next.id),
            meta: metaMap.get(next.id),
          });
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
          meta={metaMap.get(toolBlock.id)}
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

// ── Message timestamp ─────────────────────────────────────────────────────────

/** Short clock time for a transcript record: `HH:MM` for today, `MM-DD HH:MM`
 *  otherwise. Absolute (not relative) so rendered rows never go stale. */
function formatMsgTime(ts: string): { short: string; full: string } | null {
  const d = new Date(ts);
  if (isNaN(d.getTime())) return null;
  const pad = (n: number) => String(n).padStart(2, "0");
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  const now = new Date();
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  return {
    short: sameDay ? hm : `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hm}`,
    full: d.toLocaleString(),
  };
}

// ── Single message row ────────────────────────────────────────────────────────

interface MsgProps {
  msg: RawMessage;
  resultMap: Map<string, ToolResultBlock>;
  metaMap: Map<string, unknown>;
  decisionRecords: DecisionHistoryRecord[];
  searchTerms?: string[] | null;
  msgIdx?: number;
}

const MessageRow = memo(function MessageRow({ msg, resultMap, metaMap, decisionRecords, searchTerms, msgIdx }: MsgProps) {
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

  const time = msg.timestamp ? formatMsgTime(msg.timestamp) : null;

  // Turn status. Previously a timeline dot in the gutter; now a marker on the
  // usage row, because roles are told apart by layout (full-width assistant vs.
  // right-aligned user bubble) rather than by a gutter rail.
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
      <div className={styles.content}>
        {isAssistant && Array.isArray(content) && (
          <ContentBlocks
            content={content}
            resultMap={resultMap}
            metaMap={metaMap}
            decisionRecords={decisionRecords}
            isPartial={isPartial}
            searchTerms={searchTerms}
          />
        )}
        {isUser && (
          <div className={styles.user_text}>
            <UserContent
              content={content}
              renderText={(text, key) =>
                searchTerms ? (
                  <HighlightedText key={key} text={text} terms={searchTerms} />
                ) : (
                  text
                )
              }
            />
          </div>
        )}
        {isAssistant && (msg.message.usage || time) && (
          <div className={styles.usage}>
            {dotClass && <span className={`${styles.dot} ${dotClass}`} />}
            {msg.message.usage && (
              <>
                ↑{msg.message.usage.input_tokens} ↓{msg.message.usage.output_tokens}
                {msg.message.model && (
                  <span className={styles.model}> · {msg.message.model}</span>
                )}
              </>
            )}
            {time && (
              <span className={styles.msg_time} title={time.full}>
                {msg.message.usage ? " · " : ""}
                {time.short}
              </span>
            )}
          </div>
        )}
        {isUser && time && (
          <div className={styles.usage}>
            <span className={styles.msg_time} title={time.full}>
              {time.short}
            </span>
          </div>
        )}
      </div>
    </div>
  );
});

// ── Waiting for input indicator ───────────────────────────────────────────────

function WaitingIndicator() {
  const { t } = useTranslation();
  return (
    <div className={styles.waiting}>
      <span className={styles.waiting_dot} />
      {t("detail.waiting_input", "Waiting for input")}
    </div>
  );
}

// ── Live activity indicator ───────────────────────────────────────────────────

/** Session statuses that mean the agent is actively doing something right now
 *  (as opposed to waiting for the user). Mirrors the scanner's working set. */
const WORKING_STATUSES = new Set([
  "thinking", "executing", "streaming", "processing", "active", "delegating",
]);

/** Animated "still working" indicator pinned under the newest message while
 *  the session is in a working status. */
function WorkingIndicator({ status }: { status: string }) {
  const { t } = useTranslation();
  const label =
    status === "thinking"
      ? t("detail.working_thinking", "Thinking…")
      : status === "executing"
        ? t("detail.working_executing", "Running tools…")
        : status === "streaming"
          ? t("detail.working_streaming", "Responding…")
          : status === "delegating"
            ? t("detail.working_delegating", "Delegating to subagents…")
            : t("detail.working_processing", "Processing…");
  return (
    <div className={styles.working}>
      <span className={styles.working_dots}>
        <span />
        <span />
        <span />
      </span>
      {label}
    </div>
  );
}

// ── MessageList ───────────────────────────────────────────────────────────────

interface Props {
  messages: RawMessage[];
  isLoading: boolean;
  searchQuery?: string | null;
  /** Live scanner status of the session (e.g. "thinking", "executing");
   *  drives the animated activity indicator under the newest message. */
  status?: string | null;
  /** Decision records for this session. Inline decision cards read them for the
   *  asset id an image-bearing `fleet__ask` needs to re-serve its preview. */
  decisionRecords?: DecisionHistoryRecord[];
  /** Pull older messages from disk. Awaited, so the reveal happens after the
   *  fetch lands. Omit when the caller has no deeper history to offer. */
  onLoadEarlier?: () => Promise<void> | void;
  /** True when `messages` already reaches the start of the transcript. */
  fullyLoaded?: boolean;
  /** True while `onLoadEarlier` is in flight. */
  isLoadingEarlier?: boolean;
}

/** Messages revealed per click, and the initial size of the render window. */
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

const NO_DECISIONS: DecisionHistoryRecord[] = [];

export function MessageList({
  messages,
  isLoading,
  searchQuery,
  status,
  decisionRecords,
  onLoadEarlier,
  fullyLoaded = true,
  isLoadingEarlier = false,
}: Props) {
  const { t } = useTranslation();
  const listRef = useRef<HTMLDivElement>(null);
  const bottomRef = useRef<HTMLDivElement>(null);
  // Stable identity so memoized MessageRows don't re-render when the caller
  // omits the prop.
  const records = decisionRecords ?? NO_DECISIONS;

  // How many trailing messages to render. Counting from the *end* rather than
  // holding a start index is what lets this window coexist with the store's
  // disk pagination: loading earlier history prepends messages, which shifts
  // every index but leaves a count-from-the-end untouched, so the visible set
  // does not jump under the reader.
  const [visibleCount, setVisibleCount] = useState(PAGE_SIZE);
  // Saved before revealing more; used by useLayoutEffect to restore scroll position
  const scrollAnchor = useRef<{ scrollTop: number; scrollHeight: number } | null>(null);

  const resultMap = useMemo(() => buildResultMap(messages), [messages]);
  const metaMap = useMemo(() => buildToolResultMetaMap(messages), [messages]);

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

      // Widen the window far enough that the first search hit — plus a little
      // context above it — is actually inside it.
      setVisibleCount(
        searchMatchIndex >= 0
          ? visibleCountForMatch(displayMsgs.length, searchMatchIndex, PAGE_SIZE)
          : PAGE_SIZE,
      );
    }
    prevCountRef.current = displayMsgs.length;
  }, [displayMsgs.length, searchMatchIndex]);

  const { start: effectiveStart, hidden: hiddenCount } = windowSlice(
    displayMsgs.length,
    visibleCount,
  );
  const visibleMsgs = displayMsgs.slice(effectiveStart);

  // Keep the reader's place when the transcript grows.
  //
  // A *prepend* (older history arriving) needs nothing: the window counts from
  // the end, so the visible set is already unchanged. An *append* (the agent
  // writing a new turn) slides the window forward, dropping the oldest visible
  // message — which is what we want while the reader sits at the bottom, but
  // yanks content away once they have expanded the window to read back. So grow
  // the window by the same delta in that case, pinning its start.
  const prevFirstKeyRef = useRef<string | null>(null);
  const prevLenRef = useRef(displayMsgs.length);
  useEffect(() => {
    const firstKey = displayMsgs[0]?.uuid ?? displayMsgs[0]?.timestamp ?? null;
    const prepended = prevFirstKeyRef.current !== null && firstKey !== prevFirstKeyRef.current;
    const delta = displayMsgs.length - prevLenRef.current;
    prevFirstKeyRef.current = firstKey;
    prevLenRef.current = displayMsgs.length;
    setVisibleCount((v) => nextVisibleCount({ prepended, delta, visibleCount: v, pageSize: PAGE_SIZE }));
  }, [displayMsgs]);

  // After revealing more rows, restore scroll so the viewport doesn't jump
  useLayoutEffect(() => {
    const anchor = scrollAnchor.current;
    if (!anchor || !listRef.current) return;
    const scroller = listRef.current.parentElement;
    if (scroller) {
      scroller.scrollTop = anchor.scrollTop + (listRef.current.scrollHeight - anchor.scrollHeight);
    }
    scrollAnchor.current = null;
  });

  const anchorScroll = useCallback(() => {
    const scroller = listRef.current?.parentElement;
    if (scroller && listRef.current) {
      scrollAnchor.current = {
        scrollTop: scroller.scrollTop,
        scrollHeight: listRef.current.scrollHeight,
      };
    }
  }, []);

  /**
   * The one control for reaching older messages.
   *
   * Two mechanisms used to be exposed as two identical-looking buttons: this
   * component's render window, and the store's disk pagination. They are now
   * stacked behind a single action — reveal what is already in memory, and only
   * go to disk once the window has caught up with what was fetched.
   */
  const canReveal = hiddenCount > 0;
  const canFetch = !fullyLoaded && !!onLoadEarlier;
  const loadEarlier = useCallback(async () => {
    if (scrollAnchor.current || isLoadingEarlier) return;
    if (canReveal) {
      anchorScroll();
      setVisibleCount((v) => v + PAGE_SIZE);
      return;
    }
    if (!canFetch) return;
    await onLoadEarlier?.();
    // The fetch prepends, leaving the visible set unchanged; widen to reveal it.
    anchorScroll();
    setVisibleCount((v) => v + PAGE_SIZE);
  }, [canReveal, canFetch, onLoadEarlier, isLoadingEarlier, anchorScroll]);

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

  // Only take over the panel when there is genuinely nothing to show. The same
  // `isLoading` flag is raised while fetching *older* history, and blanking the
  // whole conversation for that is how the previous "load earlier" button made
  // the transcript flash empty on every click.
  if (isLoading && displayMsgs.length === 0) {
    return <div className={styles.loading}>{t("loading", "Loading…")}</div>;
  }

  return (
    <div ref={listRef} className={styles.list}>
      {(canReveal || canFetch) && (
        <button
          className={styles.load_earlier}
          onClick={loadEarlier}
          disabled={isLoadingEarlier}
        >
          {isLoadingEarlier
            ? t("detail.loading_earlier")
            : `↑ ${t("detail.load_earlier")}`}
        </button>
      )}
      {visibleMsgs.map((msg, i) => (
        <MessageRow
          key={msg.uuid ?? (effectiveStart + i)}
          msg={msg}
          resultMap={resultMap}
          metaMap={metaMap}
          decisionRecords={records}
          searchTerms={searchTerms}
          msgIdx={effectiveStart + i}
        />
      ))}
      {status && WORKING_STATUSES.has(status) ? (
        <WorkingIndicator status={status} />
      ) : (
        isWaiting && <WaitingIndicator />
      )}
      <div ref={bottomRef} />
    </div>
  );
}
