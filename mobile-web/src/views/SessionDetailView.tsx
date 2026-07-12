// Stack-navigation session detail page — the mobile counterpart of the
// desktop HistoryView detail column. Messages arrive by polling the `tail`
// relay method (no watcher push over the relay); live thinking polls its own
// sidecar method while the session is working.

import { useCallback, useEffect, useRef, useState } from "react";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
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
  return d.toLocaleTimeString("zh-CN", { hour: "2-digit", minute: "2-digit" });
}

function userText(msg: RawMessage): string {
  const parts: string[] = [];
  for (const b of blocksOf(msg)) {
    if (b.type === "text" && b.text) parts.push(b.text);
    else if (b.type === "image") parts.push("[图片]");
  }
  return parts.join("\n\n").trim();
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

  // ── Message tail polling (only while the 消息 tab is showing) ─────────
  useEffect(() => {
    if (!client || tab !== "messages") return;
    let cancelled = false;
    let timer = 0;
    const poll = async () => {
      try {
        const rows = await client.request<RawMessage[]>("tail", {
          path: session.jsonlPath,
          n: tailN,
        });
        if (!cancelled) {
          setMessages(rows);
          setLoadError(null);
        }
      } catch (e) {
        if (!cancelled && messages === null) {
          setLoadError(e instanceof Error ? e.message : "加载失败");
        }
      }
      if (!cancelled) timer = window.setTimeout(poll, TAIL_POLL_MS);
    };
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

  const rows = (messages ?? []).filter(isRenderableRow);
  const mainRows = rows.filter((m) => !m.isSidechain);

  return (
    <div className={styles.page}>
      <header className={styles.header}>
        <button className={styles.backButton} onClick={onBack}>
          ‹ 返回
        </button>
        <div className={styles.headerText}>
          <div className={styles.headerTitle}>{session.aiTitle || session.slug || "会话"}</div>
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
            {label}
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
        {messages === null && !loadError && <div className={styles.hint}>加载消息中…</div>}
        {loadError && <div className={styles.hint}>消息加载失败：{loadError}</div>}
        {messages !== null && (messages.length >= tailN || tailN > TAIL_INITIAL) && (
          <button
            className={styles.loadMore}
            onClick={() => {
              stickToBottom.current = false;
              setTailN((n) => n + TAIL_STEP);
            }}
          >
            加载更早的消息
          </button>
        )}
        {mainRows.map((msg, i) => {
          if (msg.type === "user") {
            const text = userText(msg);
            if (!text) return null;
            return (
              <div key={i} className={styles.userRow}>
                <div className={styles.userBubble}>{text}</div>
                <div className={styles.rowTime}>{fmtTime(msg.timestamp)}</div>
              </div>
            );
          }
          // assistant: thinking / text / tool_use blocks in order
          const blocks = blocksOf(msg);
          if (blocks.length === 0) return null;
          return (
            <div key={i} className={styles.assistantRow}>
              {blocks.map((b, j) => {
                if (b.type === "thinking" && b.thinking?.trim()) {
                  const key = i * 1000 + j;
                  const open = expandedThinking.has(key);
                  return (
                    <div key={j} className={styles.thinkingBlock} data-open={open}>
                      <button className={styles.thinkingToggle} onClick={() => toggleThinking(key)}>
                        ✳ 思考 {open ? "▾" : "▸"}
                      </button>
                      {open && <div className={styles.thinkingBody}>{b.thinking}</div>}
                    </div>
                  );
                }
                if (b.type === "text" && b.text?.trim()) {
                  return (
                    <div key={j} className={styles.markdown}>
                      <ReactMarkdown
                        remarkPlugins={[remarkGfm]}
                        components={{
                          a: ({ children }) => <span className={styles.mdLink}>{children}</span>,
                        }}
                      >
                        {b.text}
                      </ReactMarkdown>
                    </div>
                  );
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
              <div className={styles.rowTime}>{fmtTime(msg.timestamp)}</div>
            </div>
          );
        })}
        {liveThinking && (
          <div className={styles.liveThinking}>
            <div className={styles.liveThinkingHead}>
              <span className={styles.livePulse} />✳ 正在思考…
            </div>
            <div className={styles.liveThinkingBody}>{liveThinking.thinking}</div>
          </div>
        )}
        {messages !== null && mainRows.length === 0 && !liveThinking && (
          <div className={styles.hint}>暂无可显示的消息</div>
        )}
      </div>
      )}

      {tab === "messages" && canResumeSession(session) && (
        <ResumeComposer session={session} client={client} />
      )}
    </div>
  );
}
