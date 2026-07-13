// Stack-navigation session detail page — the mobile counterpart of the
// desktop HistoryView detail column. Messages arrive by polling the `tail`
// relay method (no watcher push over the relay); live thinking polls its own
// sidecar method while the session is working.

import { memo, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Bot, ChevronDown, ChevronLeft, ChevronRight, MessageSquareDashed, Sparkles } from "lucide-react";
import { EmptyState } from "./EmptyState";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import { dateLocale, t } from "../i18n";
import { CopyButton } from "./CopyButton";
import type { RelayClient } from "../relay";
import type {
  ContentBlock,
  LiveThinking,
  RawMessage,
  SessionInfo,
  SessionStatus,
} from "../types";
import { canResumeSession } from "../types";
import { ResumeComposer } from "./Composer";
import {
  basename,
  parseTaskNotification,
  taskTitle,
  type ParsedTaskNotification,
} from "./taskNotification";
import {
  DecisionHistoryTab,
  HandoffTab,
  TaskPlansTab,
  TokenTab,
  WorkflowTab,
} from "./SessionDetailTabs";
import styles from "./SessionDetailView.module.css";

const TAIL_POLL_MS = 2500;
const TAIL_INITIAL = 120;
const TAIL_STEP = 200;
const LIVE_THINKING_POLL_MS = 1200;
const WORKING: SessionStatus[] = ["thinking", "executing", "streaming", "processing", "delegating"];

/** A user record whose content is only tool_result blocks is a tool's output,
 *  not a user turn — same filter as the desktop's isRenderableRow. */
function isRenderableRow(msg: RawMessage): boolean {
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

function blocksOf(msg: RawMessage): ContentBlock[] {
  const content = msg.message?.content;
  if (typeof content === "string") {
    return content.trim() ? [{ type: "text", text: content }] : [];
  }
  return Array.isArray(content) ? content : [];
}

/** The one input field a tool chip shows — same list the desktop cards pick from. */
const TOOL_SUMMARY_FIELDS = ["command", "file_path", "pattern", "path", "query", "url", "skill"];

function toolSummary(block: ContentBlock): string {
  for (const field of TOOL_SUMMARY_FIELDS) {
    const v = block.input?.[field];
    if (typeof v === "string" && v) return v;
  }
  return "";
}

function fmtTime(timestamp?: string): string {
  if (!timestamp) return "";
  const d = new Date(timestamp);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleTimeString(dateLocale(), { hour: "2-digit", minute: "2-digit" });
}

interface TailDelta {
  lines: RawMessage[];
  newOffset: number;
}

/** Append freshly-tailed lines, dropping any whose uuid we already hold —
 *  the bootstrap window and the first delta can overlap by a few lines. */
function appendUnique(prev: RawMessage[], lines: RawMessage[]): RawMessage[] {
  const recent = new Set(
    prev
      .slice(-50)
      .map((m) => (m as { uuid?: string }).uuid)
      .filter(Boolean),
  );
  const fresh = lines.filter((m) => {
    const u = (m as { uuid?: string }).uuid;
    return !u || !recent.has(u);
  });
  return fresh.length > 0 ? [...prev, ...fresh] : prev;
}

function userText(msg: RawMessage): string {
  const parts: string[] = [];
  for (const b of blocksOf(msg)) {
    if (b.type === "text" && b.text) parts.push(b.text);
    else if (b.type === "image") parts.push(t("[图片]"));
  }
  return parts.join("\n\n").trim();
}

function TaskNotificationCard({ data }: { data: ParsedTaskNotification }) {
  const [open, setOpen] = useState(true);
  const raw = (data.status ?? "").toLowerCase();
  const done = raw === "completed" || raw === "success" || raw === "done";
  const failed = raw === "failed" || raw === "error" || raw === "cancelled";
  const statusCls = done ? styles.tnDone : failed ? styles.tnError : styles.tnNeutral;
  const statusLabel = done
    ? t("已完成")
    : raw === "failed" || raw === "error"
      ? t("失败")
      : raw === "cancelled"
        ? t("已取消")
        : data.status;
  const hasBody = !!data.result;
  return (
    <div className={styles.tnCard}>
      <button
        className={styles.tnHeader}
        onClick={() => hasBody && setOpen((o) => !o)}
        style={hasBody ? undefined : { cursor: "default" }}
      >
        <Bot size={15} className={styles.tnIcon} />
        <span className={styles.tnTitle}>{taskTitle(data.summary)}</span>
        {statusLabel && <span className={`${styles.tnBadge} ${statusCls}`}>{statusLabel}</span>}
        {hasBody && (open ? <ChevronDown size={14} /> : <ChevronRight size={14} />)}
      </button>
      {hasBody && open && (
        <div className={styles.tnBody}>
          <LazyMarkdown text={data.result!} bare />
        </div>
      )}
      {data.outputFile && (
        <div className={styles.tnMeta} title={data.outputFile}>
          📄 {basename(data.outputFile)}
        </div>
      )}
    </div>
  );
}

type DetailTab = "messages" | "decisions" | "plans" | "token" | "workflow" | "handoff";

const TABS: Array<[DetailTab, string]> = [
  ["messages", "消息"],
  ["decisions", "决策"],
  ["plans", "计划"],
  ["token", "Token"],
  ["workflow", "Workflow"],
  ["handoff", "接力"],
];

interface Props {
  session: SessionInfo;
  client: RelayClient | null;
  onBack: () => void;
  /** Fired after a 2s dwell — marks the session read (same as the desktop). */
  onDwellRead?: () => void;
}

/** ReactMarkdown + remarkGfm parse is heavy; mounting a few hundred of them
 *  synchronously froze the mobile webview when a long session opened. Since the
 *  view auto-scrolls to the bottom, only the last screenful is on screen — so
 *  off-screen rows render as cheap plain text and upgrade to real markdown once
 *  they scroll within 400px of the viewport. Once upgraded, they stay upgraded. */
function LazyMarkdown({ text, bare }: { text: string; bare?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  const [rich, setRich] = useState(false);
  useEffect(() => {
    if (rich) return;
    const el = ref.current;
    if (!el) return;
    // No IntersectionObserver (very old webview) → upgrade immediately.
    if (typeof IntersectionObserver === "undefined") {
      setRich(true);
      return;
    }
    const io = new IntersectionObserver(
      (entries) => {
        if (entries.some((e) => e.isIntersecting)) setRich(true);
      },
      { rootMargin: "400px 0px" },
    );
    io.observe(el);
    return () => io.disconnect();
  }, [rich]);
  return (
    // `bare` keeps the typography rules (both classes stay on the element) but
    // drops the assistant bubble's border/background so it can nest inside a
    // task-notification card without a doubled frame.
    <div ref={ref} className={bare ? `${styles.markdown} ${styles.markdownFlat}` : styles.markdown}>
      {rich ? (
        <ReactMarkdown
          remarkPlugins={[remarkGfm]}
          components={{
            a: ({ children }) => <span className={styles.mdLink}>{children}</span>,
          }}
        >
          {text}
        </ReactMarkdown>
      ) : (
        <div className={styles.mdPlaceholder}>{text}</div>
      )}
    </div>
  );
}

interface MessageRowProps {
  msg: RawMessage;
  index: number;
  /** Set of open thinking-block keys (index*1000+blockIndex). Reference is
   *  stable across the 2.5s tail poll, so `memo` skips untouched rows then;
   *  it only changes on a user toggle, when re-rendering every row is fine. */
  expandedThinking: Set<number>;
  onToggleThinking: (key: number) => void;
}

/** One conversation row. Memoized so appending new tailed lines every 2.5s
 *  doesn't re-render (and re-parse the markdown of) the rows already on screen —
 *  the long-session jank the desktop never hit because it doesn't poll. */
const MessageRow = memo(function MessageRow({
  msg,
  index,
  expandedThinking,
  onToggleThinking,
}: MessageRowProps) {
  if (msg.type === "user") {
    const text = userText(msg);
    if (!text) return null;
    // A subagent-completion notice renders as a card, not a raw-XML bubble.
    const notif = parseTaskNotification(text);
    if (notif) {
      return (
        <div className={styles.assistantRow}>
          <TaskNotificationCard data={notif} />
          <div className={styles.rowTime}>{fmtTime(msg.timestamp)}</div>
        </div>
      );
    }
    return (
      <div className={styles.userRow}>
        <div className={styles.userBubble}>{text}</div>
        <div className={styles.rowTime}>
          <CopyButton text={text} />
          {fmtTime(msg.timestamp)}
        </div>
      </div>
    );
  }
  // assistant: thinking / text / tool_use blocks in order
  const blocks = blocksOf(msg);
  if (blocks.length === 0) return null;
  const assistantText = blocks
    .filter((b) => b.type === "text" && b.text?.trim())
    .map((b) => b.text)
    .join("\n\n");
  return (
    <div className={styles.assistantRow}>
      {blocks.map((b, j) => {
        if (b.type === "thinking" && b.thinking?.trim()) {
          const key = index * 1000 + j;
          const open = expandedThinking.has(key);
          return (
            <div key={j} className={styles.thinkingBlock} data-open={open}>
              <button className={styles.thinkingToggle} onClick={() => onToggleThinking(key)}>
                <Sparkles size={13} />
                {t("思考")}
                {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
              </button>
              {open && <div className={styles.thinkingBody}>{b.thinking}</div>}
            </div>
          );
        }
        if (b.type === "text" && b.text?.trim()) {
          return <LazyMarkdown key={j} text={b.text} />;
        }
        if (b.type === "tool_use") {
          const summary = toolSummary(b);
          return (
            <div key={j} className={styles.toolChip}>
              <span className={styles.toolName}>{b.name}</span>
              {summary && <span className={styles.toolSummary}>{summary}</span>}
            </div>
          );
        }
        return null;
      })}
      <div className={styles.rowTime}>
        {assistantText && <CopyButton text={assistantText} />}
        {fmtTime(msg.timestamp)}
      </div>
    </div>
  );
});

export function SessionDetailView({ session, client, onBack, onDwellRead }: Props) {
  const [tab, setTab] = useState<DetailTab>("messages");
  const dwellFired = useRef(false);

  useEffect(() => {
    dwellFired.current = false;
    const timer = window.setTimeout(() => {
      if (!dwellFired.current) {
        dwellFired.current = true;
        onDwellRead?.();
      }
    }, 2000);
    return () => window.clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session.id]);
  const [messages, setMessages] = useState<RawMessage[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [tailN, setTailN] = useState(TAIL_INITIAL);
  const [liveThinking, setLiveThinking] = useState<LiveThinking | null>(null);
  const [expandedThinking, setExpandedThinking] = useState<Set<number>>(new Set());
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stickToBottom = useRef(true);
  const working = WORKING.includes(session.status);

  // ── Message polling (only while the 消息 tab is showing) ──────────────
  //
  // Incremental model: bootstrap = locate the file end (`tail_delta` without
  // offset) + one full `tail` for the initial window; steady state = poll
  // `tail_delta` from the last offset and append only new lines. A desktop
  // that predates `tail_delta` fails the bootstrap probe once and we fall
  // back to the v2 full-tail poll.
  const offsetRef = useRef<number | null>(null);
  const legacyRef = useRef(false);
  useEffect(() => {
    if (!client || tab !== "messages") return;
    let cancelled = false;
    let timer = 0;

    const fullTail = async () => {
      const rows = await client.request<RawMessage[]>("tail", {
        path: session.jsonlPath,
        n: tailN,
      });
      if (!cancelled) {
        setMessages(rows);
        setLoadError(null);
      }
    };

    const bootstrap = async () => {
      try {
        const loc = await client.request<TailDelta>("tail_delta", { path: session.jsonlPath });
        offsetRef.current = loc.newOffset;
      } catch {
        legacyRef.current = true; // old desktop — keep full polling
      }
      await fullTail();
    };

    const poll = async () => {
      try {
        if (legacyRef.current) {
          await fullTail();
        } else if (offsetRef.current == null) {
          await bootstrap();
        } else {
          const d = await client.request<TailDelta>("tail_delta", {
            path: session.jsonlPath,
            offset: offsetRef.current,
          });
          if (!cancelled) {
            if (d.newOffset < offsetRef.current) {
              offsetRef.current = null; // rotated/truncated → re-bootstrap
            } else {
              offsetRef.current = d.newOffset;
              if (d.lines.length > 0) {
                setMessages((prev) => appendUnique(prev ?? [], d.lines));
              }
            }
          }
        }
      } catch (e) {
        if (!cancelled && messages === null) {
          setLoadError(e instanceof Error ? e.message : t("加载失败"));
        }
      }
      if (!cancelled) timer = window.setTimeout(poll, TAIL_POLL_MS);
    };

    offsetRef.current = null; // path / window size changed → re-bootstrap
    void poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, session.jsonlPath, tailN, tab]);

  // ── Live thinking polling (only while the turn looks in progress) ─────
  useEffect(() => {
    if (!client || !working || tab !== "messages") {
      setLiveThinking(null);
      return;
    }
    let cancelled = false;
    let timer = 0;
    const poll = async () => {
      try {
        const lt = await client.request<LiveThinking | null>("live_thinking", {
          sessionId: session.id,
        });
        if (!cancelled) setLiveThinking(lt && lt.streaming ? lt : null);
      } catch {
        // agent offline — keep whatever we had
      }
      if (!cancelled) timer = window.setTimeout(poll, LIVE_THINKING_POLL_MS);
    };
    void poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
    };
  }, [client, session.id, working, tab]);

  // ── Auto-scroll: stick to bottom unless the user scrolled up ──────────
  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  }, []);

  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [messages, liveThinking]);

  const toggleThinking = useCallback((idx: number) => {
    setExpandedThinking((prev) => {
      const next = new Set(prev);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      return next;
    });
  }, []);

  const rows = useMemo(() => (messages ?? []).filter(isRenderableRow), [messages]);
  const mainRows = useMemo(() => rows.filter((m) => !m.isSidechain), [rows]);

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <button className={styles.backButton} onClick={onBack}>
          <ChevronLeft size={18} />
          {t("返回")}
        </button>
        <div className={styles.headerText}>
          <div className={styles.headerTitle}>{session.aiTitle || session.slug || t("会话")}</div>
          <div className={styles.headerSub}>{session.workspaceName}</div>
        </div>
        <span className={styles.statusDot} data-working={working} />
      </header>

      <nav className={styles.tabBar}>
        {TABS.map(([key, label]) => (
          <button
            key={key}
            className={styles.tabButton}
            data-active={tab === key}
            onClick={() => setTab(key)}
          >
            {t(label)}
          </button>
        ))}
      </nav>

      {tab === "decisions" && (
        <div className={styles.tabScroll}>
          <DecisionHistoryTab session={session} client={client} />
        </div>
      )}
      {tab === "plans" && (
        <div className={styles.tabScroll}>
          <TaskPlansTab session={session} client={client} />
        </div>
      )}
      {tab === "token" && (
        <div className={styles.tabScroll}>
          <TokenTab session={session} client={client} />
        </div>
      )}
      {tab === "workflow" && (
        <div className={styles.tabScroll}>
          <WorkflowTab session={session} client={client} />
        </div>
      )}
      {tab === "handoff" && (
        <div className={styles.tabScroll}>
          <HandoffTab session={session} client={client} />
        </div>
      )}

      {tab === "messages" && (
      <div className={styles.scroll} ref={scrollRef} onScroll={onScroll}>
        {messages === null && !loadError && <div className={styles.hint}>{t("加载消息中…")}</div>}
        {loadError && <div className={styles.hint}>{t("消息加载失败：{0}", loadError)}</div>}
        {messages !== null && (messages.length >= tailN || tailN > TAIL_INITIAL) && (
          <button
            className={styles.loadMore}
            onClick={() => {
              stickToBottom.current = false;
              setTailN((n) => n + TAIL_STEP);
            }}
          >
            {t("加载更早的消息")}
          </button>
        )}
        {mainRows.map((msg, i) => (
          <MessageRow
            key={i}
            msg={msg}
            index={i}
            expandedThinking={expandedThinking}
            onToggleThinking={toggleThinking}
          />
        ))}
        {liveThinking && (
          <div className={styles.liveThinking}>
            <div className={styles.liveThinkingHead}>
              <span className={styles.livePulse} />
              <Sparkles size={13} />
              {t("正在思考…")}
            </div>
            <div className={styles.liveThinkingBody}>{liveThinking.thinking}</div>
          </div>
        )}
        {messages !== null && mainRows.length === 0 && !liveThinking && (
          <EmptyState compact icon={MessageSquareDashed} title={t("暂无可显示的消息")} />
        )}
      </div>
      )}

      {tab === "messages" && canResumeSession(session) && (
        <ResumeComposer session={session} client={client} />
      )}
    </div>
  );
}
