// Stack-navigation session detail page — the mobile counterpart of the
// desktop HistoryView detail column. Messages arrive by polling the `tail`
// relay method (no watcher push over the relay); live thinking polls its own
// sidecar method while the session is working.

import { memo, useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import {
  Bot,
  ChevronDown,
  ChevronLeft,
  ChevronRight,
  CircleCheck,
  CircleHelp,
  Clock,
  Cog,
  FileText,
  Globe,
  ListTodo,
  MessageSquareDashed,
  Pencil,
  Puzzle,
  Search,
  Sparkles,
  Terminal,
  Waypoints,
  Wrench,
} from "lucide-react";
import { EmptyState } from "./EmptyState";
import { isFleetTool } from "./fleetTools";
import { fleetSummary } from "./FleetBody";
import ReactMarkdown from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { MermaidBlock } from "../markdown/MermaidBlock";
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
import { canResumeSession, canEnqueueSession } from "../types";
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
import { parseSkillInjection } from "../skillInjection";
import { groupMetaRuns } from "./metaGrouping";
import { countSteps, groupWorkRuns, isDecisionTool, workRunTitle } from "./workRuns";
import { friendlyToolName, toolSummary } from "./toolSummary";
import { fmtTokens, shortModelName, turnUsageByIndex } from "./turnUsage";
import { ToolDetailPanel } from "./ToolDetailPanel";
import type { ToolDigest } from "../types";
import { AgentNavProvider, useAgentNav } from "./AgentNavContext";
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
    // A block that ships a thumbnail renders as an inline image instead of the
    // "[图片]" placeholder — only thumb-less images still degrade to text.
    else if (b.type === "image" && !thumbSrc(b)) parts.push(t("[图片]"));
  }
  return parts.join("\n\n").trim();
}

/** data: URI for a block's server-side thumbnail, if the relay shipped one. */
function thumbSrc(b: ContentBlock): string | null {
  if (!b._thumb || !b.source?.data) return null;
  return `data:${b.source.media_type ?? "image/jpeg"};base64,${b.source.data}`;
}

function imageThumbs(msg: RawMessage): string[] {
  return blocksOf(msg)
    .map(thumbSrc)
    .filter((s): s is string => !!s);
}

/** Per-tool metadata harvested from the (non-renderable) tool_result rows:
 *  the relay's `_digest` stats, the error bit and result-screenshot thumbs,
 *  keyed by tool_use_id for the matching tool chip. */
interface ToolMeta {
  digest?: ToolDigest;
  isError?: boolean;
  thumbs?: string[];
}

function collectToolMeta(messages: RawMessage[]): Map<string, ToolMeta> {
  const map = new Map<string, ToolMeta>();
  for (const msg of messages) {
    for (const b of blocksOf(msg)) {
      if (b.type !== "tool_result" || !b.tool_use_id) continue;
      const meta: ToolMeta = {};
      if (b._digest) meta.digest = b._digest;
      if (b.is_error) meta.isError = true;
      if (b._thumbs?.length) {
        meta.thumbs = b._thumbs.map((d) => `data:image/jpeg;base64,${d}`);
      }
      if (meta.digest || meta.isError || meta.thumbs) map.set(b.tool_use_id, meta);
    }
  }
  return map;
}

/** A follow-up the user just submitted, echoed as a user bubble while
 *  `claude --resume` cold-starts — dropped once the real transcript row lands. */
interface OptimisticSend {
  id: string;
  text: string;
}

/** Synthetic `user` RawMessage from an optimistic send, so it flows through the
 *  same `isRenderableRow` / `groupMetaRuns` / MessageRow path as real rows. */
function optimisticToMessage(o: OptimisticSend): RawMessage {
  return {
    type: "user",
    uuid: o.id,
    timestamp: new Date().toISOString(),
    message: { role: "user", content: [{ type: "text", text: o.text }] },
  } as RawMessage;
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

/**
 * A collapsed fold for synthetic `isMeta` user turns — the SKILL.md body a
 * `Skill` load injects, or codex's developer-role boilerplate (sandbox/
 * permissions preamble, the `/root` multi-agent collaboration prompt,
 * `<multi_agent_mode>` guidance, tagged `isMeta` in codex_source.rs). Neither is
 * user-authored. A single turn self-labels from its body (a skill load reads as
 * a skill, everything else as system context); a run of adjacent turns collapses
 * into one card that shows the count and expands to each self-labelled segment.
 */
function MetaFoldCard({ segments }: { segments: string[] }) {
  const [open, setOpen] = useState(false);
  const merged = segments.length > 1;
  const first = segments[0] ?? "";
  const skill = merged ? null : parseSkillInjection(first);
  const Icon = skill ? Puzzle : Cog;
  const label = skill ? t("已加载 SKILL") : t("系统上下文");
  const tag = merged
    ? t("{0} 条", segments.length)
    : skill
      ? skill.slug
      : deriveMetaLabel(first);
  return (
    <div className={styles.skillCard}>
      <button className={styles.skillHeader} onClick={() => setOpen((o) => !o)}>
        <Icon size={14} className={styles.skillIcon} />
        <span className={styles.skillLabel}>{label}</span>
        <span className={styles.skillSlug}>{tag}</span>
        {open ? <ChevronDown size={14} /> : <ChevronRight size={14} />}
      </button>
      {open && (
        <div className={styles.skillBody}>
          {segments.map((seg, i) => (
            <div key={i} className={merged ? styles.skillSegment : undefined}>
              {merged && <div className={styles.skillSegHead}>{deriveMetaLabel(seg)}</div>}
              <LazyMarkdown text={seg} bare />
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

/** Short tag for the fold header so the reader can tell which codex injection
 * it is without expanding; falls back to a generic label. */
function deriveMetaLabel(body: string): string {
  const head = body.slice(0, 200);
  if (head.startsWith("<permissions instructions>")) return t("权限 / 沙箱");
  if (head.includes("primary agent in a team")) return t("多智能体协作");
  if (head.startsWith("<multi_agent_mode>")) return "multi_agent_mode";
  if (head.startsWith("<environment_context>")) return t("环境上下文");
  if (head.startsWith("<user_instructions>")) return t("用户指令");
  // Newer codex CLI emits the AGENTS.md guidance under this markdown heading
  // instead of a <user_instructions> wrapper (see codex_source.rs).
  if (head.startsWith("# AGENTS.md instructions")) return t("用户指令");
  if (head.startsWith("<turn_aborted>")) return t("轮次中断");
  if (head.startsWith("<subagent_notification>")) return t("子智能体通知");
  return t("注入指令");
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
  /** The full live session array — the lookup table for subagent drill-down
   *  (`agent-<id>` rows) and the parent breadcrumb. */
  sessions: SessionInfo[];
  client: RelayClient | null;
  onBack: () => void;
  /** Push a session id as a new drill-down layer (subagent / parent nav). */
  onOpenSessionId: (id: string) => void;
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
          remarkPlugins={mdRemarkPlugins}
          rehypePlugins={mdRehypePlugins}
          components={{
            a: ({ children }) => <span className={styles.mdLink}>{children}</span>,
            code: ({ className, children, ...rest }) =>
              /(^|\s)language-mermaid(\s|$)/.test(className ?? "") ? (
                <MermaidBlock code={String(children).replace(/\n$/, "")} />
              ) : (
                <code className={className} {...rest}>
                  {children}
                </code>
              ),
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

// ── Rail steps (mirrors the desktop's de-chromed work-block language) ────────

const EDIT_TOOLS = new Set(["Edit", "MultiEdit", "Write", "NotebookEdit", "apply_patch"]);
const SHELL_TOOLS = new Set(["Bash", "exec", "exec_command", "write_stdin"]);
const WEB_TOOLS = new Set(["WebSearch", "WebFetch"]);
const SEARCH_TOOLS = new Set(["Grep", "Glob", "Explore", "LSP"]);
const AGENT_TOOLS = new Set(["Agent", "spawn_agent", "wait_agent"]);
const PLAN_TOOLS = new Set(["TodoWrite", "TodoRead", "update_plan"]);

function railToolIcon(name: string): ReactNode {
  if (SHELL_TOOLS.has(name)) return <Terminal />;
  if (EDIT_TOOLS.has(name)) return <Pencil />;
  if (name === "Read") return <FileText />;
  if (SEARCH_TOOLS.has(name)) return <Search />;
  if (WEB_TOOLS.has(name)) return <Globe />;
  if (AGENT_TOOLS.has(name)) return <Bot />;
  if (PLAN_TOOLS.has(name)) return <ListTodo />;
  if (isDecisionTool(name)) return <CircleHelp />;
  if (isFleetTool(name)) return <Waypoints />;
  return <Wrench />;
}

function RailStep({ icon, children }: { icon: ReactNode; children: ReactNode }) {
  return (
    <div className={styles.railStep}>
      <span className={styles.railIcon} aria-hidden>
        {icon}
      </span>
      <div className={styles.railBody}>{children}</div>
    </div>
  );
}

/** Thinking as plain muted prose, clamped behind a bottom fade when it
 *  overflows; tap toggles the full text. The fade only draws when there is
 *  hidden text for it to hint at. */
function ClampedThinking({
  text,
  open,
  onToggle,
}: {
  text: string;
  open: boolean;
  onToggle: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [overflowing, setOverflowing] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (el) setOverflowing(el.scrollHeight > el.clientHeight + 1);
  }, [text, open]);
  return (
    <div
      ref={ref}
      className={styles.thinkText}
      data-clamped={!open || undefined}
      data-fade={(!open && overflowing) || undefined}
      onClick={onToggle}
    >
      {text}
    </div>
  );
}

/** Compact header stats for a tool chip, rendered from the relay's `_digest` —
 *  the mobile counterpart of the desktop toolPresenters `headerStats`. */
function DigestChips({ meta }: { meta: ToolMeta }) {
  const d = meta.digest;
  const chips: ReactNode[] = [];
  if (meta.isError) {
    chips.push(
      <span key="err" className={`${styles.chip} ${styles.chipError}`}>
        {t("错误")}
      </span>,
    );
  }
  if (d) {
    if (d.added !== undefined || d.removed !== undefined) {
      chips.push(
        <span key="diff" className={styles.chip}>
          <span className={styles.chipAdd}>+{d.added ?? 0}</span>{" "}
          <span className={styles.chipDel}>−{d.removed ?? 0}</span>
        </span>,
      );
    }
    if (d.interrupted) {
      chips.push(
        <span key="int" className={`${styles.chip} ${styles.chipError}`}>
          {t("已中断")}
        </span>,
      );
    }
    if (d.matches !== undefined) {
      chips.push(<span key="m" className={styles.chip}>{t("{0} 匹配", d.matches)}</span>);
    } else if (d.files !== undefined) {
      chips.push(<span key="f" className={styles.chip}>{t("{0} 文件", d.files)}</span>);
    }
    if (d.agentStatus) {
      chips.push(
        <span key="a" className={styles.chip}>
          {d.agentStatus}
          {d.tokens !== undefined ? ` · ↓${fmtTokens(d.tokens)}` : ""}
        </span>,
      );
    }
    if (d.links !== undefined) {
      chips.push(<span key="l" className={styles.chip}>{t("{0} 结果", d.links)}</span>);
    }
    if (d.httpCode !== undefined) {
      chips.push(<span key="h" className={styles.chip}>{d.httpCode}</span>);
    }
    if (d.todoTotal !== undefined) {
      chips.push(
        <span key="t" className={styles.chip}>{`${d.todoDone ?? 0}/${d.todoTotal}`}</span>,
      );
    }
  }
  if (chips.length === 0) return null;
  return <span className={styles.chipRow}>{chips}</span>;
}

/** A row of tappable thumbnails (result screenshots / pasted images). */
function ThumbRow({ srcs }: { srcs: string[] }) {
  return (
    <div className={styles.thumbRow}>
      {srcs.map((src, i) => (
        <img key={i} src={src} className={styles.thumbImg} alt="" loading="lazy" />
      ))}
    </div>
  );
}

/** One tool call on the rail: summary line + digest chips, tap to expand the
 *  full body (fetched on demand through the relay `tool_detail` method).
 *  Expansion state is local so it survives the parent's poll re-renders. */
function ToolStep({
  b,
  client,
  jsonlPath,
  meta,
}: {
  b: ContentBlock;
  client: RelayClient | null;
  jsonlPath?: string;
  meta?: ToolMeta;
}) {
  const [open, setOpen] = useState(false);
  const nav = useAgentNav();
  const name = b.name ?? "";
  const fleetTool = isFleetTool(name);
  const summary = fleetTool ? fleetSummary(fleetTool, b.input ?? {}) : toolSummary(b);
  const expandable = !!b.id && !!client && !!jsonlPath;
  // "打开子代理": an Agent tool whose result carries the subagent's id and whose
  // transcript the snapshot has surfaced (`agent-<id>` row present).
  const agentId = AGENT_TOOLS.has(name) ? meta?.digest?.agentId : undefined;
  const canOpenAgent = !!agentId && !!nav?.has(agentId);
  return (
    <RailStep icon={railToolIcon(name)}>
      <div
        className={styles.toolLineRow}
        onClick={expandable ? () => setOpen((o) => !o) : undefined}
        role={expandable ? "button" : undefined}
      >
        <div className={styles.toolLine} title={name}>
          {summary || friendlyToolName(name)}
        </div>
        {meta && <DigestChips meta={meta} />}
      </div>
      {canOpenAgent && (
        <button
          type="button"
          className={styles.openSubagent}
          onClick={(e) => {
            e.stopPropagation();
            nav!.open(agentId!);
          }}
        >
          {t("打开子代理")} →
        </button>
      )}
      {meta?.thumbs && <ThumbRow srcs={meta.thumbs} />}
      {open && expandable && (
        <ToolDetailPanel
          client={client}
          jsonlPath={jsonlPath!}
          toolUseId={b.id!}
          isError={meta?.isError}
        />
      )}
    </RailStep>
  );
}

/** One assistant record's blocks in the rail language: thinking and tool calls
 *  as icon-guttered steps, prose flush and full width — same convention as the
 *  desktop transcript. */
function AssistantBlocks({
  blocks,
  index,
  expandedThinking,
  onToggleThinking,
  toolMeta,
  client,
  jsonlPath,
}: {
  blocks: ContentBlock[];
  index: number;
  expandedThinking: Set<number>;
  onToggleThinking: (key: number) => void;
  toolMeta?: Map<string, ToolMeta>;
  client?: RelayClient | null;
  jsonlPath?: string;
}) {
  return (
    <>
      {blocks.map((b, j) => {
        if (b.type === "thinking" && b.thinking?.trim()) {
          const key = index * 1000 + j;
          return (
            <RailStep key={j} icon={<Clock />}>
              <ClampedThinking
                text={b.thinking}
                open={expandedThinking.has(key)}
                onToggle={() => onToggleThinking(key)}
              />
            </RailStep>
          );
        }
        if (b.type === "text" && b.text?.trim()) {
          return <LazyMarkdown key={j} text={b.text} />;
        }
        if (b.type === "image") {
          const src = thumbSrc(b);
          return src ? <ThumbRow key={j} srcs={[src]} /> : null;
        }
        if (b.type === "tool_use") {
          return (
            <ToolStep
              key={j}
              b={b}
              client={client ?? null}
              jsonlPath={jsonlPath}
              meta={b.id ? toolMeta?.get(b.id) : undefined}
            />
          );
        }
        return null;
      })}
    </>
  );
}

/** Inline markdown for the work-run band headline: `p` unwraps to a fragment
 *  so a one-sentence title renders inline (no block paragraph) inside the
 *  nowrap/ellipsis span, while `**bold**`/`code` still resolve. Links stay
 *  inert (same as LazyMarkdown) since a title never navigates. */
const bandTitleMdComponents = {
  p: ({ children }: { children?: ReactNode }) => <>{children}</>,
  a: ({ children }: { children?: ReactNode }) => (
    <span className={styles.mdLink}>{children}</span>
  ),
};

/**
 * A run of ≥2 adjacent pure-work records folded behind one summary line —
 * the mobile counterpart of the desktop WorkRunBlock. `live` (the run is the
 * transcript's working tail) opens the band and shimmers the headline; a
 * finished run closes with the Done check.
 */
function WorkRunBand({
  msgs,
  baseIndex,
  expandedThinking,
  onToggleThinking,
  live,
  toolMeta,
  client,
  jsonlPath,
}: {
  msgs: RawMessage[];
  baseIndex: number;
  expandedThinking: Set<number>;
  onToggleThinking: (key: number) => void;
  live: boolean;
  toolMeta?: Map<string, ToolMeta>;
  client?: RelayClient | null;
  jsonlPath?: string;
}) {
  const last = msgs[msgs.length - 1];
  // Streaming = the run's final record hasn't recorded a stop_reason yet. An
  // old relay that still strips the field (undefined) falls back to the
  // session-status approximation the band used before.
  const lastStop = last?.message && "stop_reason" in last.message
    ? last.message.stop_reason
    : undefined;
  const streaming = lastStop === undefined ? live : live && lastStop === null;
  const [open, setOpen] = useState(streaming);
  useEffect(() => setOpen(streaming), [streaming]);
  const title = workRunTitle(msgs) ?? t("处理任务");
  const bandTokens = msgs.reduce((sum, m) => sum + (m.message?.usage?.output_tokens ?? 0), 0);
  return (
    <div className={styles.assistantRow}>
      <button className={styles.bandHeader} onClick={() => setOpen((o) => !o)}>
        {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        <span className={`${styles.bandTitle}${streaming ? ` ${styles.shimmer}` : ""}`}>
          {/* The headline is thinking-derived and often carries markdown emphasis
              (`**Planning store test additions**`); render it inline (p unwrapped
              to a fragment) so markers become bold/italic/code, not literal
              asterisks, while staying on one line inside the nowrap/ellipsis span. */}
          <ReactMarkdown
            remarkPlugins={mdRemarkPlugins}
            rehypePlugins={mdRehypePlugins}
            components={bandTitleMdComponents}
          >
            {title}
          </ReactMarkdown>
        </span>
        <span className={styles.bandSteps}>
          {t("{0} 步", countSteps(msgs))}
          {bandTokens > 0 && ` · ↓${fmtTokens(bandTokens)}`}
        </span>
      </button>
      {open && (
        <div className={styles.bandBody}>
          {msgs.map((m, i) => (
            <AssistantBlocks
              key={(m as { uuid?: string }).uuid ?? i}
              blocks={blocksOf(m)}
              index={baseIndex + i}
              expandedThinking={expandedThinking}
              onToggleThinking={onToggleThinking}
              toolMeta={toolMeta}
              client={client}
              jsonlPath={jsonlPath}
            />
          ))}
          {!streaming && (
            <div className={`${styles.railStep} ${styles.doneStep}`}>
              <span className={`${styles.railIcon} ${styles.doneIcon}`} aria-hidden>
                <CircleCheck />
              </span>
              <div className={styles.doneLabel}>{t("完成")}</div>
            </div>
          )}
        </div>
      )}
      <div className={styles.rowTime}>{fmtTime(last?.timestamp)}</div>
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
  /** This row's tool metadata (digest chips / error bits / result thumbs).
   *  Rebuilt every poll, so the memo comparator diffs it by content. */
  toolMeta?: Map<string, ToolMeta>;
  /** Aggregated usage when this row closes an assistant turn. */
  turnUsage?: { inputTokens: number; outputTokens: number; model?: string };
  /** For the tap-to-expand tool_detail fetch; both are stable per session. */
  client?: RelayClient | null;
  jsonlPath?: string;
}

/** Content equality for the per-row tool metadata — reference equality would
 *  defeat the row memo on every poll (the maps are rebuilt each tick even when
 *  nothing about this row changed). The payloads are tiny digests, so a JSON
 *  compare is cheap and exact. */
function toolMetaEqual(a?: Map<string, ToolMeta>, b?: Map<string, ToolMeta>): boolean {
  if (a === b) return true;
  if (!a || !b || a.size !== b.size) return false;
  for (const [k, v] of a) {
    const other = b.get(k);
    if (!other || JSON.stringify(v) !== JSON.stringify(other)) return false;
  }
  return true;
}

/** One conversation row. Memoized so appending new tailed lines every 2.5s
 *  doesn't re-render (and re-parse the markdown of) the rows already on screen —
 *  the long-session jank the desktop never hit because it doesn't poll. */
const MessageRow = memo(function MessageRow({
  msg,
  index,
  expandedThinking,
  onToggleThinking,
  toolMeta,
  turnUsage,
  client,
  jsonlPath,
}: MessageRowProps) {
  if (msg.type === "user") {
    const text = userText(msg);
    const thumbs = imageThumbs(msg);
    if (!text && thumbs.length === 0) return null;
    // Every synthetic `isMeta` user turn — a SKILL.md body a `Skill` load
    // injects, or codex's developer-role boilerplate — is harness/runtime
    // content, not a user turn. Fold them all into one card that self-labels
    // from its body rather than a raw user bubble.
    if (msg.isMeta) {
      return (
        <div className={styles.assistantRow}>
          <MetaFoldCard segments={[text]} />
          <div className={styles.rowTime}>{fmtTime(msg.timestamp)}</div>
        </div>
      );
    }
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
        <div className={styles.userBubble}>
          {thumbs.length > 0 && <ThumbRow srcs={thumbs} />}
          {text && <LazyMarkdown text={text} bare />}
        </div>
        <div className={styles.rowTime}>
          {text && <CopyButton text={text} />}
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
      <AssistantBlocks
        blocks={blocks}
        index={index}
        expandedThinking={expandedThinking}
        onToggleThinking={onToggleThinking}
        toolMeta={toolMeta}
        client={client}
        jsonlPath={jsonlPath}
      />
      <div className={styles.rowTime}>
        {assistantText && <CopyButton text={assistantText} />}
        {turnUsage && (
          <span className={styles.usageLine}>
            ↑{fmtTokens(turnUsage.inputTokens)} ↓{fmtTokens(turnUsage.outputTokens)}
            {turnUsage.model ? ` · ${shortModelName(turnUsage.model)}` : ""}
            {" · "}
          </span>
        )}
        {fmtTime(msg.timestamp)}
      </div>
    </div>
  );
},
(prev, next) =>
  prev.msg === next.msg &&
  prev.index === next.index &&
  prev.expandedThinking === next.expandedThinking &&
  prev.onToggleThinking === next.onToggleThinking &&
  prev.client === next.client &&
  prev.jsonlPath === next.jsonlPath &&
  toolMetaEqual(prev.toolMeta, next.toolMeta) &&
  JSON.stringify(prev.turnUsage ?? null) === JSON.stringify(next.turnUsage ?? null));

export function SessionDetailView({
  session,
  sessions,
  client,
  onBack,
  onOpenSessionId,
  onDwellRead,
}: Props) {
  const [tab, setTab] = useState<DetailTab>("messages");
  const dwellFired = useRef(false);
  // Subagent drill-down nav (same table-lookup model as the desktop): resolve
  // `agent-<id>` in the live session array; `open` pushes it as a new layer.
  const nav = useMemo(
    () => ({
      open: (agentId: string) => onOpenSessionId(`agent-${agentId}`),
      has: (agentId: string) => sessions.some((s) => s.id === `agent-${agentId}`),
    }),
    [sessions, onOpenSessionId],
  );
  // When viewing a subagent, the owning main session (for the parent breadcrumb).
  const parentSession = useMemo(
    () =>
      session.isSubagent && session.parentSessionId
        ? (sessions.find((s) => s.id === session.parentSessionId) ?? null)
        : null,
    [session.isSubagent, session.parentSessionId, sessions],
  );

  useEffect(() => {
    dwellFired.current = false;
    // Never carry one session's pending echo (or a stuck in-flight flag) over.
    setOptimisticSends([]);
    submitInFlightRef.current = false;
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
  // Optimistic follow-ups: echoed the moment the desktop acks a resume, dropped
  // once the real row arrives via tail. Kept out of `messages` so the poller's
  // setMessages doesn't clobber them.
  const [optimisticSends, setOptimisticSends] = useState<OptimisticSend[]>([]);
  const optimisticSeq = useRef(0);
  // True while a resume/enqueue submit is in flight — the tail / live-thinking
  // pollers skip their tick then, yielding the single serialized WS to the
  // resume req/reply rather than contending with a big tail response.
  const submitInFlightRef = useRef(false);
  const handleSubmitInFlight = useCallback((v: boolean) => {
    submitInFlightRef.current = v;
  }, []);
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
      // Paused while the tab is hidden: no point waking the CPU every few
      // seconds to tail a view nobody is looking at. Returning without
      // rescheduling stops the timer chain entirely; `onVisible` restarts it on
      // return to the foreground. (Matches the visibility gate in App.tsx.)
      if (document.visibilityState !== "visible") return;
      // A resume/enqueue submit is on the wire — don't fire a competing tail on
      // the single serialized WS; the optimistic echo already shows the user's
      // message, so a skipped tick costs nothing. Retry on the next interval.
      if (submitInFlightRef.current) {
        if (!cancelled) timer = window.setTimeout(poll, TAIL_POLL_MS);
        return;
      }
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
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      window.clearTimeout(timer); // drop any stray timer → single chain
      void poll();
    };
    document.addEventListener("visibilitychange", onVisible);
    void poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
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
      // Paused while hidden — see the tail poller above. This is the tightest
      // loop in the app (1.2s), so gating it on visibility is the biggest
      // single battery win. `onVisible` restarts it on foreground.
      if (document.visibilityState !== "visible") return;
      // Same yield as the tail poller: hold off while a submit is in flight.
      if (submitInFlightRef.current) {
        if (!cancelled) timer = window.setTimeout(poll, LIVE_THINKING_POLL_MS);
        return;
      }
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
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      window.clearTimeout(timer); // drop any stray timer → single chain
      void poll();
    };
    document.addEventListener("visibilitychange", onVisible);
    void poll();
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
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

  // tool_detail reads the Claude jsonl by tool_use_id; a codex rollout has no
  // toolUseResult and its folded format defeats the scan — leave codex tool
  // lines non-expandable (digest chips never arrive for codex either).
  const detailPath = session.agentSource === "codex" ? undefined : session.jsonlPath;

  // Text of every real user row, to tell which optimistic echoes have landed.
  const realUserTexts = useMemo(() => {
    const set = new Set<string>();
    for (const m of messages ?? []) {
      if (m.type === "user") set.add(userText(m));
    }
    return set;
  }, [messages]);

  const pendingOptimistic = useMemo(
    () => optimisticSends.filter((o) => !realUserTexts.has(o.text)),
    [optimisticSends, realUserTexts],
  );

  // Once an echo lands in the real transcript, prune it (dedup by trimmed text).
  useEffect(() => {
    setOptimisticSends((prev) => {
      const next = prev.filter((o) => !realUserTexts.has(o.text));
      return next.length === prev.length ? prev : next;
    });
  }, [realUserTexts]);

  const handleOptimisticSend = useCallback((text: string) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    optimisticSeq.current += 1;
    setOptimisticSends((prev) => [
      ...prev,
      { id: `optimistic-${Date.now()}-${optimisticSeq.current}`, text: trimmed },
    ]);
  }, []);

  const mainRows = useMemo(() => {
    const base = rows.filter((m) => !m.isSidechain);
    if (pendingOptimistic.length === 0) return base;
    return [...base, ...pendingOptimistic.map(optimisticToMessage)];
  }, [rows, pendingOptimistic]);

  // Tool metadata lives on the tool_result rows the renderable filter drops —
  // harvest it from the unfiltered list, keyed by tool_use_id.
  const toolMetaMap = useMemo(() => collectToolMeta(messages ?? []), [messages]);
  const turnUsage = useMemo(() => turnUsageByIndex(mainRows), [mainRows]);

  // Per-row subset so the row memo can diff by content instead of re-rendering
  // every row each poll (the full map is rebuilt every tick).
  const metaForMsg = useCallback(
    (msg: RawMessage): Map<string, ToolMeta> | undefined => {
      let m: Map<string, ToolMeta> | undefined;
      for (const b of blocksOf(msg)) {
        if (b.type === "tool_use" && b.id) {
          const meta = toolMetaMap.get(b.id);
          if (meta) (m ??= new Map()).set(b.id, meta);
        }
      }
      return m;
    },
    [toolMetaMap],
  );

  return (
    <AgentNavProvider nav={nav}>
    <div className={styles.page}>
      <header className={styles.header}>
        <button className={styles.backButton} onClick={onBack}>
          <ChevronLeft size={18} />
          {t("返回")}
        </button>
        <div className={styles.headerText}>
          <div className={styles.headerTitle}>
            {session.isSubagent && (
              <span className={styles.subagentBadge}>⎇ {session.agentType || t("子代理")}</span>
            )}
            {session.titleOverride || session.aiTitle || session.slug || t("会话")}
          </div>
          <div className={styles.headerSub}>{session.workspaceName}</div>
        </div>
        <span className={styles.statusDot} data-working={working} />
      </header>

      {session.isSubagent && (
        <button
          type="button"
          className={styles.parentCrumb}
          disabled={!parentSession}
          onClick={() => session.parentSessionId && onOpenSessionId(session.parentSessionId)}
        >
          ↑ {t("来自")}{" "}
          {parentSession
            ? parentSession.titleOverride ||
              parentSession.aiTitle ||
              parentSession.slug ||
              t("父会话")
            : t("父会话")}
        </button>
      )}

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
        {(() => {
          const units = groupWorkRuns(groupMetaRuns(mainRows));
          return units.map((unit, unitIdx) => {
            if (unit.kind === "work-group") {
              // A run of pure-work records folds behind one summary line; the
              // working tail's run opens itself and shimmers its headline.
              const live = working && unitIdx === units.length - 1;
              return (
                <WorkRunBand
                  key={unit.startLocal}
                  msgs={unit.msgs}
                  baseIndex={unit.startLocal}
                  expandedThinking={expandedThinking}
                  onToggleThinking={toggleThinking}
                  live={live}
                  toolMeta={toolMetaMap}
                  client={client}
                  jsonlPath={detailPath}
                />
              );
            }
            if (unit.kind === "meta-group") {
              // One card for the whole run of adjacent system-context turns.
              const segments = unit.msgs.map(userText).filter(Boolean);
              if (segments.length === 0) return null;
              const last = unit.msgs[unit.msgs.length - 1];
              return (
                <div key={unit.startLocal} className={styles.assistantRow}>
                  <MetaFoldCard segments={segments} />
                  <div className={styles.rowTime}>{fmtTime(last.timestamp)}</div>
                </div>
              );
            }
            return (
              <MessageRow
                key={unit.startLocal}
                msg={unit.msg}
                index={unit.startLocal}
                expandedThinking={expandedThinking}
                onToggleThinking={toggleThinking}
                toolMeta={metaForMsg(unit.msg)}
                turnUsage={turnUsage.get(unit.startLocal)}
                client={client}
                jsonlPath={detailPath}
              />
            );
          });
        })()}
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
        <ResumeComposer
          session={session}
          client={client}
          onOptimisticSend={handleOptimisticSend}
          onSubmitInFlight={handleSubmitInFlight}
        />
      )}
      {tab === "messages" && canEnqueueSession(session) && (
        <ResumeComposer
          session={session}
          client={client}
          mode="enqueue"
          onSubmitInFlight={handleSubmitInFlight}
        />
      )}
    </div>
    </AgentNavProvider>
  );
}
