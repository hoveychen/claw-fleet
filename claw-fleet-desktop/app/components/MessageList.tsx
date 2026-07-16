import { Fragment, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, memo } from "react";
import { useTranslation } from "react-i18next";
import type {
  DecisionHistoryRecord,
  RawMessage,
  ToolResultBlock,
} from "../types";
import { buildToolResultMetaMap } from "../toolResults";
import { nextVisibleCount, visibleCountForMatch, windowSlice } from "../messageWindow";
import {
  dayKey,
  daysAgo,
  findMatches,
  isRenderableRow,
  messageSearchText,
  messageToText,
  shortModelName,
  turnUsageByRow,
  type TurnUsage,
} from "../messageRows";
import type { PathLinkContext } from "../markdown/pathLinks";
import { ContentBlocks } from "./blocks/ContentBlocks";
import { WorkRunBlock } from "./blocks/WorkRunBlock";
import { invoke } from "@tauri-apps/api/core";
import {
  ToolResultFetchContext,
  type ToolResultFetch,
  type FullToolResult,
} from "./blocks/toolResultFetch";
import { UserContent } from "./blocks/UserContent";
import { CopyButton } from "./CopyButton";
import { CompactSummaryBlock } from "./blocks/CompactSummaryBlock";
import { MetaFoldBlock } from "./blocks/MetaFoldBlock";
import { groupMetaRuns } from "./metaGrouping";
import { groupWorkRuns } from "./workRuns";
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
  /** The hit the search navigation is currently parked on. */
  isActiveMatch?: boolean;
  paths?: PathLinkContext;
  /** The turn's aggregated usage when this row is its final assistant record;
   *  null on mid-turn rows, which render no usage line at all (that line
   *  repeating under every record was the loudest noise in tool-heavy turns). */
  turnUsage?: TurnUsage | null;
}

const MessageRow = memo(function MessageRow({ msg, resultMap, metaMap, decisionRecords, searchTerms, msgIdx, isActiveMatch, paths, turnUsage }: MsgProps) {
  // Same predicate the list uses to build its window and day separators, so the
  // two can never disagree about which records occupy a row.
  if (!isRenderableRow(msg) || !msg.message) return null;

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

  // Every synthetic `isMeta` user turn — the SKILL.md body a `Skill` load
  // injects, or codex's developer-role boilerplate (sandbox/permissions
  // preamble, the `/root` collaboration prompt, `<multi_agent_mode>`) — is
  // harness/runtime content, not something the user typed. Fold them all into
  // one collapsed divider that self-labels from its body, instead of letting
  // them fall through to a full-width user bubble.
  if (isUser && msg.isMeta) {
    return (
      <div className={styles.compact_row} data-msg-idx={msgIdx}>
        <MetaFoldBlock segments={[messageToText(msg)]} />
      </div>
    );
  }

  const isPartial =
    isAssistant && msg.message.stop_reason === null;

  const time = msg.timestamp ? formatMsgTime(msg.timestamp) : null;
  const copyText = messageToText(msg);

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
      className={`${styles.message} ${isAssistant ? styles.assistant : styles.user} ${isActiveMatch ? styles.active_match : ""}`}
      data-msg-idx={msgIdx}
    >
      {/* Tool-only assistant turns have no prose worth copying. */}
      {copyText && (
        <div className={styles.row_actions}>
          <CopyButton text={copyText} />
        </div>
      )}
      <div className={styles.content}>
        {isAssistant && Array.isArray(content) && (
          <ContentBlocks
            content={content}
            resultMap={resultMap}
            metaMap={metaMap}
            decisionRecords={decisionRecords}
            isPartial={isPartial}
            searchTerms={searchTerms}
            paths={paths}
          />
        )}
        {isUser && (
          <div className={styles.user_text}>
            <UserContent
              content={content}
              paths={paths}
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
        {isAssistant && turnUsage && (
          <div className={styles.usage}>
            {dotClass && <span className={`${styles.dot} ${dotClass}`} />}
            ↑{turnUsage.inputTokens} ↓{turnUsage.outputTokens}
            {msg.message.model && (
              <span className={styles.model}> · {shortModelName(msg.message.model)}</span>
            )}
            {time && (
              <span className={styles.msg_time} title={time.full}>
                {" · "}
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

// ── Day separator ─────────────────────────────────────────────────────────────

/**
 * Sticky date header shown above the first row of each day.
 *
 * The topmost visible row always gets one, so a reader who scrolled into the
 * middle of an old session can still see which day they are looking at.
 */
function DaySeparator({ isoDay }: { isoDay: string }) {
  const { t } = useTranslation();
  const ago = daysAgo(isoDay, new Date());
  const label =
    ago === 0
      ? t("detail.today")
      : ago === 1
        ? t("detail.yesterday")
        : new Date(`${isoDay}T00:00:00`).toLocaleDateString(undefined, {
            year: "numeric",
            month: "short",
            day: "numeric",
          });
  return (
    <div className={styles.day_sep}>
      <span className={styles.day_sep_label}>{label}</span>
    </div>
  );
}

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
  /** Workspace context that turns path-shaped inline code into clickable
   *  chips. Callers that know the session's workspace pass it; InspectModal
   *  and other context-free callers leave it out and paths stay inert. */
  paths?: PathLinkContext;
  /** The session's transcript path. Lets a tool card recover the full output
   *  the tail payload truncated (`get_tool_result_full`). Omit and truncated
   *  cards show only their inline preview. */
  jsonlPath?: string;
}

/** Messages revealed per click, and the initial size of the render window. */
const PAGE_SIZE = 100;


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
  paths,
  jsonlPath,
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

  // tool_use_ids whose output the backend truncated for transport. Their cards
  // fetch the full body on expand (see toolResultFetch + Rust message_trim).
  const truncatedIds = useMemo(() => {
    const ids = new Set<string>();
    for (const msg of messages) {
      if (!msg._fleetTruncated) continue;
      const content = msg.message?.content;
      if (!Array.isArray(content)) continue;
      for (const block of content) {
        if (block.type === "tool_result") ids.add((block as ToolResultBlock).tool_use_id);
      }
    }
    return ids;
  }, [messages]);

  // Provide on-demand full-output recovery only when we have both a jsonl path
  // to fetch from and at least one truncated card — otherwise leave the context
  // null so cards render their inline content unchanged.
  const toolFetch = useMemo<ToolResultFetch | null>(() => {
    if (!jsonlPath || truncatedIds.size === 0) return null;
    return {
      truncatedIds,
      fetchFull: (toolUseId: string) =>
        invoke<FullToolResult>("get_tool_result_full", { jsonlPath, toolUseId }),
    };
  }, [jsonlPath, truncatedIds]);

  // Only records that actually occupy a row. Excluding the 32% of user records
  // that carry nothing but tool results keeps the render window honest (a
  // "100 message" window used to show ~67 rows) and stops a day separator from
  // landing above an invisible message.
  const displayMsgs = useMemo(() => messages.filter(isRenderableRow), [messages]);

  // One `↑ ↓ · model · time` line per turn, on its final assistant row.
  const turnUsage = useMemo(() => turnUsageByRow(displayMsgs), [displayMsgs]);

  // Parse search terms once for matching and highlighting
  const searchTerms = useMemo(() => {
    if (!searchQuery || searchQuery.trim().length < 2) return null;
    return searchQuery.trim().toLowerCase().split(/\s+/).filter(Boolean);
  }, [searchQuery]);

  // Search haystacks are built from the messages alone, so typing another term
  // re-scans strings instead of re-serialising every tool input.
  const haystacks = useMemo(() => displayMsgs.map(messageSearchText), [displayMsgs]);
  const matches = useMemo(
    () => (searchTerms ? findMatches(haystacks, searchTerms) : []),
    [haystacks, searchTerms],
  );

  // Which hit the reader is currently parked on.
  const [activeMatch, setActiveMatch] = useState(0);
  useEffect(() => {
    setActiveMatch(0);
  }, [searchTerms, displayMsgs.length === 0]);

  const searchMatchIndex = matches.length > 0 ? matches[Math.min(activeMatch, matches.length - 1)] : -1;

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

  // Collapse runs of adjacent synthetic `isMeta` user turns into a single fold,
  // so N back-to-back injected turns don't stack N identical "系统上下文" dividers.
  // A second pass folds runs of tool-call/thinking assistant records into
  // WorkRunBlock bands. Grouping lives at the list level — a MessageRow only
  // sees one message.
  const renderUnits = groupWorkRuns(groupMetaRuns(visibleMsgs));

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

  /** Scroll a rendered row into the middle of the viewport, if it is mounted. */
  const scrollToRow = useCallback((msgIdx: number) => {
    const row = listRef.current?.querySelector(`[data-msg-idx="${msgIdx}"]`);
    row?.scrollIntoView({ behavior: "smooth", block: "center" });
  }, []);

  /**
   * Step to the next or previous hit, wrapping around.
   *
   * A hit above the render window is unreachable until the window covers it, so
   * widen first; the scroll then happens on the next frame, once the row exists.
   */
  const stepMatch = useCallback(
    (delta: number) => {
      if (matches.length === 0) return;
      const next = (activeMatch + delta + matches.length) % matches.length;
      setActiveMatch(next);
      const target = matches[next];
      setVisibleCount((v) =>
        Math.max(v, visibleCountForMatch(displayMsgs.length, target, PAGE_SIZE)),
      );
      requestAnimationFrame(() => scrollToRow(target));
    },
    [matches, activeMatch, displayMsgs.length, scrollToRow],
  );

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
  // Keeps the trailing work band open while the agent is live in it, so the
  // reader watches tools stream in; it folds itself once the agent moves on.
  const isWorkingNow = !!status && WORKING_STATUSES.has(status);

  // Only take over the panel when there is genuinely nothing to show. The same
  // `isLoading` flag is raised while fetching *older* history, and blanking the
  // whole conversation for that is how the previous "load earlier" button made
  // the transcript flash empty on every click.
  if (isLoading && displayMsgs.length === 0) {
    return <div className={styles.loading}>{t("loading", "Loading…")}</div>;
  }

  return (
    <ToolResultFetchContext.Provider value={toolFetch}>
    <div ref={listRef} className={`${styles.list} ${searchTerms ? styles.list_searching : ""}`}>
      {searchTerms && (
        <div className={styles.search_nav}>
          <span className={styles.search_count}>
            {matches.length === 0
              ? t("detail.search_no_match")
              : `${activeMatch + 1} / ${matches.length}`}
          </span>
          <button
            type="button"
            className={styles.search_step}
            onClick={() => stepMatch(-1)}
            disabled={matches.length === 0}
            title={t("detail.search_prev")}
            aria-label={t("detail.search_prev")}
          >
            ↑
          </button>
          <button
            type="button"
            className={styles.search_step}
            onClick={() => stepMatch(1)}
            disabled={matches.length === 0}
            title={t("detail.search_next")}
            aria-label={t("detail.search_next")}
          >
            ↓
          </button>
        </div>
      )}
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
      {renderUnits.map((unit, unitIdx) => {
        const i = unit.startLocal;
        // A separator opens each new day, and also the top of the window — a
        // reader scrolled into the middle of an old session needs the date too.
        // Keyed off the unit's first row; meta and work groups never straddle a day.
        const today = dayKey(visibleMsgs[i].timestamp);
        const prev = i > 0 ? dayKey(visibleMsgs[i - 1].timestamp) : null;
        const showDay = today !== null && (i === 0 || today !== prev);
        const globalStart = effectiveStart + i;
        if (unit.kind === "work-group") {
          // Same anchor scheme as the meta fold: every folded record keeps a
          // `data-msg-idx` so search navigation can land on the band.
          return (
            <Fragment key={visibleMsgs[i].uuid ?? globalStart}>
              {showDay && <DaySeparator isoDay={today} />}
              <div className={styles.compact_row} data-msg-idx={globalStart}>
                {unit.msgs.slice(1).map((_, k) => (
                  <span key={k} data-msg-idx={globalStart + 1 + k} aria-hidden />
                ))}
                <WorkRunBlock
                  msgs={unit.msgs}
                  resultMap={resultMap}
                  metaMap={metaMap}
                  decisionRecords={records}
                  searchTerms={searchTerms}
                  paths={paths}
                  defaultOpen={isWorkingNow && unitIdx === renderUnits.length - 1}
                />
              </div>
            </Fragment>
          );
        }
        if (unit.kind === "meta-group") {
          // One divider for the whole run. Each folded turn still carries its own
          // `data-msg-idx` anchor (empty spans) so search-navigation can scroll to
          // any member — they all resolve to the collapsed card's position.
          return (
            <Fragment key={visibleMsgs[i].uuid ?? globalStart}>
              {showDay && <DaySeparator isoDay={today} />}
              <div className={styles.compact_row} data-msg-idx={globalStart}>
                {unit.msgs.slice(1).map((_, k) => (
                  <span key={k} data-msg-idx={globalStart + 1 + k} aria-hidden />
                ))}
                <MetaFoldBlock segments={unit.msgs.map(messageToText)} />
              </div>
            </Fragment>
          );
        }
        return (
          <Fragment key={unit.msg.uuid ?? globalStart}>
            {showDay && <DaySeparator isoDay={today} />}
            <MessageRow
              msg={unit.msg}
              resultMap={resultMap}
              metaMap={metaMap}
              decisionRecords={records}
              searchTerms={searchTerms}
              msgIdx={globalStart}
              isActiveMatch={globalStart === searchMatchIndex}
              paths={paths}
              turnUsage={turnUsage.get(globalStart) ?? null}
            />
          </Fragment>
        );
      })}
      {status && WORKING_STATUSES.has(status) ? (
        <WorkingIndicator status={status} />
      ) : (
        isWaiting && <WaitingIndicator />
      )}
      <div ref={bottomRef} />
    </div>
    </ToolResultFetchContext.Provider>
  );
}
