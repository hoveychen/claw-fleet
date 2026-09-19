// Stack-navigation session detail page — the mobile counterpart of the
// desktop HistoryView detail column. Messages arrive by polling the `tail`
// relay method (no watcher push over the relay); live thinking polls its own
// sidecar method while the session is working.

import {
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  Bot,
  ChevronDown,
  ChevronRight,
  CircleCheck,
  CircleHelp,
  Clock,
  Cog,
  FileText,
  Globe,
  ListTodo,
  LoaderCircle,
  MessageSquareDashed,
  MoreHorizontal,
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
import { IngestCard, ingestStepLabel } from "./IngestCard";
import { fleetSummary } from "./FleetBody";
import ReactMarkdown from "react-markdown";
import { mdRemarkPlugins, mdRehypePlugins } from "../markdown/plugins";
import { mdComponents } from "../markdown/components";
import { dateLocale, t } from "../i18n";
import { CopyButton } from "./CopyButton";
import { useLightbox } from "./Lightbox";
import { AttachmentThumbs } from "./AttachmentThumb";
import { splitContextFiles } from "../userAttachments";
import type { FleetTransport } from "../transport";
import type {
  ContentBlock,
  LiveThinking,
  RawMessage,
  SessionInfo,
  SessionStatus,
} from "../types";
import { canResumeSession, canEnqueueSession } from "../types";
import { detailPathForSession } from "../agentSource";
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
  NotesTab,
  TaskPlansTab,
  TokenTab,
  WorkflowTab,
} from "./SessionDetailTabs";
import { parseSkillInjection } from "../skillInjection";
import { groupMetaRuns } from "./metaGrouping";
import { countSteps, groupWorkRuns, isDecisionTool, workRunFinished, workRunTitle } from "./workRuns";
import { decisionSummary, friendlyToolName, toolSummary } from "./toolSummary";
import {
  InFlightToolsContext,
  inFlightToolIds,
  isBackgroundShell,
  useInFlightTools,
} from "./inFlightTools";
import { userDisplayText } from "./slashCommand";
import { fmtTokens, shortModelName, turnUsageByIndex } from "./turnUsage";
import { ToolDetailPanel } from "./ToolDetailPanel";
import type { IngestSummary, ToolDigest } from "../types";
import { memberDisplayStatus } from "../../../shared-ts/memberStatus";
import { AgentNavProvider, useAgentNav } from "./AgentNavContext";
import { HistoryLayer } from "../useNavStack";
import { SessionSheet } from "./SessionSheet";
import { StatusRail } from "./StatusRail";
import { buildStatusPills, type DetailPane, type PillTarget } from "./sessionStatusPills";
import { filterMainRows } from "./mainRows";
import styles from "./SessionDetailView.module.css";
import { AppHeader } from "./AppHeader";
import { FleetEventCard } from "./FleetEventCard";
import { ApiErrorCard } from "./ApiErrorCard";
import { classifySyntheticError } from "../../../shared-ts/syntheticError";

const TAIL_POLL_MS = 2500;
const TAIL_INITIAL = 120;
const TAIL_STEP = 200;
const LIVE_THINKING_POLL_MS = 1200;
const WORKING: SessionStatus[] = ["thinking", "executing", "streaming", "processing", "delegating"];

/** Max subagents listed in the scope switcher — a parent that fanned out
 *  dozens/hundreds would otherwise flood the dropdown. Mirrors the desktop
 *  SUBAGENT_TAB_CAP. The menu scrolls, so the capped set stays reachable. */
const SUBAGENT_TAB_CAP = 12;

/** Statuses that count as "this member is doing something", used to order the
 *  scope switcher active-first. Mirrors the desktop LIVE_STATUSES (broader than
 *  WORKING: includes waitingInput/active) so a main session parked for input
 *  still sorts ahead of finished ones. Consulted through
 *  `memberDisplayStatus`, which is what keeps a *subagent* at `waitingInput`
 *  (= its final report landed) from sorting as active. A subagent genuinely
 *  parked on a decision card reads Executing (stop_reason=tool_use), so it is
 *  unaffected. */
const SCOPE_LIVE: Set<SessionStatus> = new Set([
  "thinking",
  "executing",
  "streaming",
  "processing",
  "waitingInput",
  "active",
  "delegating",
]);

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

const INTERRUPT_MARKERS = new Set([
  "[Request interrupted by user]",
  "[Request interrupted by user for tool use]",
]);

/** The sole text a record carries, or null when it has none / carries more.
 *  Mirrors the desktop's `messageRows.soleText`. */
function soleText(msg: RawMessage): string | null {
  const blocks = blocksOf(msg);
  if (blocks.length !== 1 || blocks[0].type !== "text") return null;
  return blocks[0].text ?? null;
}

/** Claude persists Esc/interrupt — and Fleet's SIGINT when a Decision Card
 *  times out — as a synthetic user turn. A terminal marker for the turn above,
 *  not a prompt anyone typed, so it renders as a hairline rule. */
export function isInterruptMarker(msg: RawMessage): boolean {
  if (msg.type !== "user") return false;
  const text = soleText(msg);
  return text !== null && INTERRUPT_MARKERS.has(text);
}

/** Filler Claude writes when a turn is aborted before it said anything. Other
 *  `<synthetic>` records (a 403 auth failure) carry real news and stay. */
export function isNoResponseFiller(msg: RawMessage): boolean {
  if (msg.type !== "assistant" || msg.message?.model !== "<synthetic>") return false;
  return soleText(msg)?.trim() === "No response requested.";
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
  /** Set on the two calls that file something into a store, so the row can show
   *  the deliverable instead of a bare "产出" (Artifact) chip. */
  ingest?: IngestSummary;
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
      if (b._ingest) meta.ingest = b._ingest;
      if (meta.digest || meta.isError || meta.thumbs || meta.ingest) map.set(b.tool_use_id, meta);
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

/** The title displayed in the header for the active pane. Of the six labels
 *  that used to sit on the old tab bar, only these five remain — "Messages" is
 *  not here because it is no longer a tab, it is this page itself. */
const PANE_TITLE: Record<DetailPane, string> = {
  decisions: "决策记录",
  plans: "计划",
  token: "Token 与花费",
  workflow: "Workflow",
  notes: "笔记",
  handoff: "接力链",
};

interface Props {
  session: SessionInfo;
  /** The full live session array — the lookup table for subagent drill-down
   *  (`agent-<id>` rows) and the parent breadcrumb. */
  sessions: SessionInfo[];
  client: FleetTransport | null;
  onBack: () => void;
  /** Push a session id as a new drill-down layer (subagent / parent nav). */
  onOpenSessionId: (id: string) => void;
  /** Count of pending decision cards for this session. Decision cards are an
   *  aggregated inbox across devices (`App`'s `aggregateDecisions`), not stored on
   *  `SessionInfo`, so the App counts them by sessionId and passes them here —
   *  the status rail at the top uses this to render "N pending decisions", the
   *  only pill that truly blocks work. */
  pendingDecisions?: number;
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
          components={mdComponents}
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
// The dsh names below are the tools with no Claude counterpart, so they are
// not renamed upstream in `dsh_messages.rs` and need their glyph here. Kept in
// step with the desktop's Rail.tsx.
const SHELL_TOOLS = new Set([
  "Bash", "exec", "exec_command", "write_stdin",
  "job_output", "job_kill", "job_list", "run_code",
  "terminal_open", "terminal_list", "terminal_read", "terminal_send",
  "terminal_close", "terminal_signal",
]);
const WEB_TOOLS = new Set(["WebSearch", "WebFetch"]);
const SEARCH_TOOLS = new Set([
  "Grep", "Glob", "Explore", "LSP",
  "session_search", "session_trace",
  "session_event_read", "session_event_search", "session_event_trace",
]);
// Mirrors the desktop Rail's set: core renames dsh's `subagent` /
// `subagent_fork` to `Agent` upstream, while `workflow` keeps its own name.
const AGENT_TOOLS = new Set([
  "Agent", "spawn_agent", "wait_agent",
  "workflow", "list_agents", "send_message", "interrupt_agent", "report",
]);
const PLAN_TOOLS = new Set([
  "TodoWrite", "TodoRead", "update_plan",
  "create_goal", "update_goal", "get_goal",
  "schedule_create", "schedule_delete", "schedule_list",
]);

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
    // A decision card's chosen answer — what the reader scrolls back to find.
    // Free text can be a whole paragraph, so the chip clamps and the expanded
    // body carries the content.
    if (d.answer) {
      chips.push(
        <span key="ans" className={`${styles.chip} ${styles.chipAnswer}`} title={d.answer}>
          {d.answer}
        </span>,
      );
    }
  }
  if (chips.length === 0) return null;
  return <span className={styles.chipRow}>{chips}</span>;
}

/** A row of tappable thumbnails (result screenshots / pasted images). Tapping
 *  opens the full-screen lightbox — note these are the low-res `_thumbs`, so
 *  the enlarged view is the preview scaled up; the tool chip's detail panel
 *  carries the full-resolution bytes. */
function ThumbRow({ srcs }: { srcs: string[] }) {
  const { open } = useLightbox();
  return (
    <div className={styles.thumbRow}>
      {srcs.map((src, i) => (
        <img
          key={i}
          src={src}
          className={styles.thumbImg}
          alt=""
          loading="lazy"
          onClick={() => open(src)}
        />
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
  client: FleetTransport | null;
  jsonlPath?: string;
  meta?: ToolMeta;
}) {
  const [open, setOpen] = useState(false);
  const nav = useAgentNav();
  const inFlight = useInFlightTools();
  // Reading this through context (not a prop) is deliberate: MessageRow is
  // memoized against the 2.5s tail poll, and a context update re-renders the
  // consumer through that memo without widening its comparator.
  const running = !!b.id && inFlight.has(b.id);
  const background = isBackgroundShell(b);
  const name = b.name ?? "";
  const fleetTool = isFleetTool(name);
  const summary = meta?.ingest
    ? // For ingestion calls, full identity lives on the card below; the line
      // shows only the action name (the relay strips title/slug, so the old
      // template is now half a sentence on mobile).
      ingestStepLabel(meta.ingest)
    : fleetTool
    ? fleetSummary(fleetTool, b.input ?? {})
    : isDecisionTool(name)
      ? decisionSummary(b)
      : name === "TaskStop"
        ? // TaskStop's input is just an opaque task_id; what was stopped lives only
          // in the result (the relay puts the command's first line into digest.stoppedCommand).
          meta?.digest?.stoppedCommand
          ? t("停止后台任务：{0}", meta.digest.stoppedCommand)
          : t("停止后台任务")
        : // TaskOutput likewise: which task we're reading from lives only in the
          // result (digest.taskDescription).
          name === "TaskOutput" && meta?.digest?.taskDescription
          ? t("读取后台任务输出：{0}", meta.digest.taskDescription)
          : toolSummary(b);
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
        {background && <span className={styles.toolBgTag}>{t("后台")}</span>}
        {running && (
          <span className={styles.toolRunning} role="status">
            {t("运行中")}
          </span>
        )}
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
      {/* Deliverables from this step appear below the step line — they are the
          **result** of this call, not scaffold records, so they are always visible
          without expanding. */}
      {meta?.ingest && <IngestCard ingest={meta.ingest} client={client} />}
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

/**
 * Stable identity of one transcript record, for React keys and for the
 * "which thinking blocks are expanded" set.
 *
 * It must not be the row's position: the list is a *tail* window, and 「加载更早
 * 的消息」 prepends 200 rows at the head, sliding every index along. Keyed by
 * position, an expanded band (or an expanded thinking block) then belongs to a
 * different message than the one the reader opened. The transcript's `uuid` is
 * the record's own identity and survives the shift; the positional fallback is
 * only for rows that have none (optimistic sends, fixtures).
 */
export function rowKeyOf(msg: RawMessage | undefined, index: number): string {
  return (msg as { uuid?: string } | undefined)?.uuid ?? `i${index}`;
}

/** One assistant record's blocks in the rail language: thinking and tool calls
 *  as icon-guttered steps, prose flush and full width — same convention as the
 *  desktop transcript. */
function AssistantBlocks({
  blocks,
  rowKey,
  expandedThinking,
  onToggleThinking,
  toolMeta,
  client,
  jsonlPath,
}: {
  blocks: ContentBlock[];
  /** Identity of the record these blocks came from — see `rowKeyOf`. */
  rowKey: string;
  expandedThinking: Set<string>;
  onToggleThinking: (key: string) => void;
  toolMeta?: Map<string, ToolMeta>;
  client?: FleetTransport | null;
  jsonlPath?: string;
}) {
  return (
    <>
      {blocks.map((b, j) => {
        if (b.type === "thinking" && b.thinking?.trim()) {
          const key = `${rowKey}#${j}`;
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
 *  inert here — unlike message bodies, this headline sits inside the band's
 *  `<button>`, so a real anchor would be interactive content nested in a
 *  control (and a title never needs to navigate). */
const bandTitleMdComponents = {
  p: ({ children }: { children?: ReactNode }) => <>{children}</>,
  a: ({ children }: { children?: ReactNode }) => (
    <span className={styles.mdLink}>{children}</span>
  ),
};

/**
 * A run of ≥2 adjacent pure-work records folded behind one summary line —
 * the mobile counterpart of the desktop WorkRunBlock. `tail` (the run is the
 * transcript's last unit) opens the band; `live` (the session is working)
 * shimmers the headline; a finished run closes with the Done check.
 */
function WorkRunBand({
  msgs,
  baseIndex,
  expandedThinking,
  onToggleThinking,
  live,
  tail,
  toolMeta,
  resultIds,
  client,
  jsonlPath,
}: {
  msgs: RawMessage[];
  baseIndex: number;
  expandedThinking: Set<string>;
  onToggleThinking: (key: string) => void;
  /** The session is working *and* this run is the trailing unit — drives the
   *  headline shimmer and withholds the Done check (the run can still grow). */
  live: boolean;
  /** This run is the transcript's trailing unit — it starts open. */
  tail: boolean;
  toolMeta?: Map<string, ToolMeta>;
  /** Ids of tool calls whose result has come back — a call missing from this
   *  set is still in flight, so the band withholds its Done check. */
  resultIds?: Set<string>;
  client?: FleetTransport | null;
  jsonlPath?: string;
}) {
  const last = msgs[msgs.length - 1];
  const finished = workRunFinished(msgs, live, (id) => resultIds?.has(id) ?? true);
  // In progress = the run can still grow, i.e. the same fact the Done check
  // reads, inverted. It used to additionally require the last record to be an
  // unterminated partial (`stop_reason === null`), which flapped the shimmer
  // off between records and for the entire time a tool was running — the same
  // stop_reason misreading that put a premature 完成 on the rail.
  const streaming = !finished && live;
  // The trailing band starts open and stays open: folded, a growing tail shows
  // only a rising step count and a newer timestamp with nothing to read. It is
  // a latch, not a mirror — `streaming` flips off mid-run whenever a tool
  // outlives the backend's freshness window, and mirroring it both ways both
  // flapped the band shut under the reader and stomped a manual toggle.
  const [open, setOpen] = useState(tail);
  useEffect(() => {
    if (tail) setOpen(true);
  }, [tail]);
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
              key={rowKeyOf(m, baseIndex + i)}
              blocks={blocksOf(m)}
              rowKey={rowKeyOf(m, baseIndex + i)}
              expandedThinking={expandedThinking}
              onToggleThinking={onToggleThinking}
              toolMeta={toolMeta}
              client={client}
              jsonlPath={jsonlPath}
            />
          ))}
          {finished && (
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
  /** Record identity (`rowKeyOf`), not a list position. */
  rowKey: string;
  /** Set of open thinking-block keys (`<rowKey>#<blockIndex>`). Reference is
   *  stable across the 2.5s tail poll, so `memo` skips untouched rows then;
   *  it only changes on a user toggle, when re-rendering every row is fine. */
  expandedThinking: Set<string>;
  onToggleThinking: (key: string) => void;
  /** This row's tool metadata (digest chips / error bits / result thumbs).
   *  Rebuilt every poll, so the memo comparator diffs it by content. */
  toolMeta?: Map<string, ToolMeta>;
  /** Aggregated usage when this row closes an assistant turn. */
  turnUsage?: { inputTokens: number; outputTokens: number; model?: string };
  /** For the tap-to-expand tool_detail fetch; both are stable per session. */
  client?: FleetTransport | null;
  jsonlPath?: string;
  /** Identity behind a failed-turn card's buttons (retry / switch model / sign
   *  in). Absent for a transcript with no live session; the card then renders
   *  its classification without buttons it could not honour. */
  session?: { id: string; workspacePath: string; agentSource?: string | null } | null;
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
  rowKey,
  expandedThinking,
  onToggleThinking,
  toolMeta,
  turnUsage,
  client,
  jsonlPath,
  session,
}: MessageRowProps) {
  if (isNoResponseFiller(msg)) return null;
  // A turn Claude Code failed out of — an expired token, a quota, a 529. It
  // carries a machine-readable `error` enum and usually exactly one way out, so
  // it gets a card with that way out on it instead of a grey assistant bubble.
  // Same classification the desktop uses (`shared-ts/syntheticError`).
  const apiError = classifySyntheticError(msg);
  if (apiError) {
    return (
      <div className={styles.assistantRow}>
        <ApiErrorCard info={apiError} session={session} client={client} />
        <div className={styles.rowTime}>{fmtTime(msg.timestamp)}</div>
      </div>
    );
  }
  if (isInterruptMarker(msg)) {
    return (
      <div className={styles.interruptRule} data-testid="interrupt-rule">
        <span className={styles.interruptRuleLabel}>{t("已中断")}</span>
      </div>
    );
  }
  if (msg.type === "user") {
    // Automation payloads must travel through the harness's user-prompt
    // channel, but they are Fleet events rather than user-authored turns.
    if (msg.fleetEvent) {
      return (
        <div className={styles.assistantRow}>
          <FleetEventCard event={msg.fleetEvent} text={userText(msg)} />
          <div className={styles.rowTime}>{fmtTime(msg.timestamp)}</div>
        </div>
      );
    }
    // The composers staple picked files onto the prompt as a trailing
    // `Context files:` block, which Claude Code freezes into the transcript
    // verbatim. Peel it back off: the paths become thumbnails, and the bubble
    // shows what the person actually typed instead of a wall of absolute paths.
    const { body: text, paths: attachments } = splitContextFiles(userText(msg));
    const thumbs = imageThumbs(msg);
    if (!text && thumbs.length === 0 && attachments.length === 0) return null;
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
    // What the person typed stays plain and whitespace-preserved — deliberately
    // *not* markdown, matching the desktop's UserContent.tsx (and claude.ai /
    // ChatGPT): measuring this repo's transcripts there said only ~6% of user
    // messages carry markdown structure while ~10% contain single newlines a
    // markdown renderer silently collapses into one paragraph. It also drops the
    // CJK first-line indent the markdown chain applied to Chinese prose, which
    // made a two-character message like 「进度」 sit two characters off its own
    // bubble's left edge.
    const display = userDisplayText(text);
    return (
      <div className={styles.userRow}>
        <div className={styles.userBubble}>
          {thumbs.length > 0 && <ThumbRow srcs={thumbs} />}
          {display && <div className={styles.userText}>{display}</div>}
          <AttachmentThumbs paths={attachments} client={client ?? null} />
        </div>
        <div className={styles.rowTime}>
          {display && <CopyButton text={display} />}
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
        rowKey={rowKey}
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
  prev.rowKey === next.rowKey &&
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
  pendingDecisions = 0,
}: Props) {
  /** The active pane — one of the five tabs that used to sit on the old bar —
   *  or `null` to stay on the message page.
   *
   *  The old code had a six-valued `tab` with `"messages"` as the default,
   *  making messages and Workflow structurally equivalent options though they
   *  were vastly unequal in use (messages are why you came to this page; the
   *  other five are occasional check-in views). Switching to `pane | null`
   *  enshrines this asymmetry in the type and lets the body escape the
   *  permanent tab bar. */
  const [pane, setPane] = useState<DetailPane | null>(null);
  /** Session detail sheet — opened by tapping the title or the ⋮ button. */
  const [sheetOpen, setSheetOpen] = useState(false);
  const openTarget = useCallback((target: PillTarget) => {
    if (target === "sheet") setSheetOpen(true);
    else setPane(target);
  }, []);
  const statusPills = useMemo(
    () => buildStatusPills(session, { pendingDecisions }),
    [session, pendingDecisions],
  );
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
  // The session family (main + its subagents) for the scope switcher. Built the
  // same way as the desktop SessionDetail: match by parentSessionId, exclude
  // workflow fan-out agents (`subagents/workflows/…`, 100+ per run), order
  // active members first then most-recently-active finished ones, and cap. A
  // running subagent's `agent-<id>` row is scanned the moment its transcript
  // file appears, so it lands here with no wait for a completed Agent result.
  // Empty when there are no subagents to switch between (solo session).
  const family = useMemo<SessionInfo[]>(() => {
    const notWorkflow = (s: SessionInfo) => !s.jsonlPath.includes("/subagents/workflows/");
    let mainSession: SessionInfo | undefined;
    let subs: SessionInfo[];
    if (session.isSubagent && session.parentSessionId) {
      mainSession = sessions.find((s) => s.id === session.parentSessionId);
      subs = sessions.filter(
        (s) => s.isSubagent && s.parentSessionId === session.parentSessionId && notWorkflow(s),
      );
    } else {
      mainSession = session;
      subs = sessions.filter(
        (s) => s.isSubagent && s.parentSessionId === session.id && notWorkflow(s),
      );
    }
    if (subs.length === 0) return [];
    const active = subs.filter((s) => SCOPE_LIVE.has(memberDisplayStatus(s)));
    const finished = subs
      .filter((s) => !SCOPE_LIVE.has(memberDisplayStatus(s)))
      .sort((a, b) => b.lastActivityMs - a.lastActivityMs);
    let ordered = [...active, ...finished].slice(0, SUBAGENT_TAB_CAP);
    // Never drop the subagent currently being viewed, even if fresher siblings
    // pushed it past the cap — its row must stay selectable.
    if (session.isSubagent && !ordered.some((s) => s.id === session.id)) {
      ordered = [session, ...ordered.slice(0, SUBAGENT_TAB_CAP - 1)];
    }
    return mainSession ? [mainSession, ...ordered] : ordered;
  }, [session, sessions]);

  useEffect(() => {
    // When drilling into a subagent, close the sheet and active pane: they
    // describe the session you just left (every chip, watch, and scope list is
    // from the previous session).
    setSheetOpen(false);
    setPane(null);
    // Never carry one session's pending echo (or a stuck in-flight flag) over.
    setOptimisticSends([]);
    submitInFlightRef.current = false;
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [session.id]);
  const [messages, setMessages] = useState<RawMessage[] | null>(null);
  const messagesRef = useRef<RawMessage[] | null>(messages);
  messagesRef.current = messages;
  const [syncingLatest, setSyncingLatest] = useState(false);
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
  const [expandedThinking, setExpandedThinking] = useState<Set<string>>(new Set());
  const scrollRef = useRef<HTMLDivElement | null>(null);
  const stickToBottom = useRef(true);
  // The composer floats above the transcript and doesn't consume layout space,
  // so the scroll area needs to yield the space it occludes. Use measured values
  // rather than hardcoding: the composer grows with input, attachments, and
  // queued messages.
  //
  // The composer is permanent; it used to auto-slide open/closed based on scroll
  // direction (48px down to collapse, 24px up to expand), but that cost a full
  // composer's worth of bottom padding every cycle — and padding is below content,
  // so scrollTop either lagged (last lines hidden under the composer) or got
  // clamped (text jumped under your finger). Both bugs lived in that mechanism,
  // so it was torn out entirely along with its DELTA/FLOOR/SETTLE constants,
  // settle window, and bounce-back safety net. Now this value only changes when
  // the composer's own content changes; scrolling never touches it.
  const [composerHeight, setComposerHeight] = useState(0);
  const working = WORKING.includes(session.status);

  // ── Message polling (only while viewing messages) ──────────────
  //
  // Incremental model: bootstrap = locate the file end (`tail_delta` without
  // offset) + one full `tail` for the initial window; steady state = poll
  // `tail_delta` from the last offset and append only new lines. A desktop
  // that predates `tail_delta` fails the bootstrap probe once and we fall
  // back to the v2 full-tail poll.
  const offsetRef = useRef<number | null>(null);
  const legacyRef = useRef(false);
  useEffect(() => {
    if (!client || pane !== null) return;
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

    const poll = async (reportSync = false) => {
      // Paused while the tab is hidden: no point waking the CPU every few
      // seconds to tail a view nobody is looking at. Returning without
      // rescheduling stops the timer chain entirely; `onVisible` restarts it on
      // return to the foreground. (Matches the visibility gate in App.tsx.)
      if (document.visibilityState !== "visible") return;
      // A resume/enqueue submit is on the wire — don't fire a competing tail on
      // the single serialized WS; the optimistic echo already shows the user's
      // message, so a skipped tick costs nothing. Retry on the next interval.
      if (submitInFlightRef.current) {
        if (!cancelled) timer = window.setTimeout(() => void poll(reportSync), TAIL_POLL_MS);
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
      if (!cancelled) {
        if (reportSync) setSyncingLatest(false);
        timer = window.setTimeout(() => void poll(), TAIL_POLL_MS);
      }
    };

    offsetRef.current = null; // path / window size changed → re-bootstrap
    const onVisible = () => {
      if (document.visibilityState !== "visible") return;
      window.clearTimeout(timer); // drop any stray timer → single chain
      const reportSync = messagesRef.current !== null;
      if (reportSync) setSyncingLatest(true);
      void poll(reportSync);
    };
    document.addEventListener("visibilitychange", onVisible);
    const reportSync = messagesRef.current !== null;
    if (reportSync) setSyncingLatest(true);
    void poll(reportSync);
    return () => {
      cancelled = true;
      window.clearTimeout(timer);
      document.removeEventListener("visibilitychange", onVisible);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, session.jsonlPath, tailN, pane]);

  // ── Live thinking polling (only while the turn looks in progress) ─────
  useEffect(() => {
    if (!client || !working || pane !== null) {
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
  }, [client, session.id, working, pane]);

  // This view is not remounted when the open session changes, and the poller
  // above deliberately keeps the last reasoning through a failed or skipped
  // sample (offline agent, hidden tab, submit in flight). Both together let one
  // session's reasoning show under another, so ownership is checked at render.
  const shownLiveThinking =
    liveThinking && liveThinking.sessionId === session.id ? liveThinking : null;

  // ── Auto-scroll: stick to bottom unless the user scrolled up ──────────
  //
  // This handler does one thing now. It used to also drive the composer's
  // auto-collapse (accumulated distance, direction reversals, settle window),
  // but that whole mechanism was torn out — see the comment at composerHeight.
  const onScroll = useCallback(() => {
    const el = scrollRef.current;
    if (!el) return;
    stickToBottom.current = el.scrollHeight - el.scrollTop - el.clientHeight < 80;
  }, []);

  // composerHeight is also in deps: the bottom padding is added **below** the
  // content, so no amount of adjusting scrollTop makes it keep up on its own.
  // The composer no longer auto-collapses with scrolling, but it still grows and
  // shrinks with input, attachments, queued messages, and collapsed decision cards;
  // when stuck to bottom, we re-stick and keep the last message in view.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [messages, shownLiveThinking, composerHeight]);

  const toggleThinking = useCallback((idx: string) => {
    setExpandedThinking((prev) => {
      const next = new Set(prev);
      if (next.has(idx)) next.delete(idx);
      else next.add(idx);
      return next;
    });
  }, []);

  const rows = useMemo(() => (messages ?? []).filter(isRenderableRow), [messages]);

  // tool_detail reads the Claude jsonl by tool_use_id; a codex rollout has no
  // toolUseResult and its folded format defeats the scan, and a dsh session has
  // no transcript file at all — its `jsonlPath` is a `dsh://` uri. Both leave
  // tool lines non-expandable (digest chips never arrive for either).
  const detailPath = detailPathForSession(session.agentSource, session.jsonlPath);

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
    const base = filterMainRows(rows, session.jsonlPath);
    if (pendingOptimistic.length === 0) return base;
    return [...base, ...pendingOptimistic.map(optimisticToMessage)];
  }, [rows, pendingOptimistic, session.jsonlPath]);

  // Tool metadata lives on the tool_result rows the renderable filter drops —
  // harvest it from the unfiltered list, keyed by tool_use_id.
  const toolMetaMap = useMemo(() => collectToolMeta(messages ?? []), [messages]);
  // Which tool calls have a result back. `toolMetaMap` can't answer this — it
  // only keeps results that carried a digest/thumb/error — and a work band
  // needs it to tell a finished run from one still waiting on its last tool.
  const resultIds = useMemo(() => {
    const ids = new Set<string>();
    for (const msg of messages ?? []) {
      for (const b of blocksOf(msg)) {
        if (b.type === "tool_result" && b.tool_use_id) ids.add(b.tool_use_id);
      }
    }
    return ids;
  }, [messages]);
  // Which of those calls are executing right now, so the step itself says so
  // instead of leaving the band's missing Done check to imply it.
  const inFlightTools = useMemo(
    () => inFlightToolIds(messages ?? [], resultIds, working, blocksOf),
    [messages, resultIds, working],
  );
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
    <InFlightToolsContext.Provider value={inFlightTools}>
    <div className={styles.page}>
      {/* `seamless`: below the header comes either the status rail (which has its
          own bottom border) or the ↑from breadcrumb — both are the same chrome
          layer; adding another border to the header would split the block in two.
          This page is why AppHeader exists: it is the one that drifted. */}
      <AppHeader
        onBack={onBack}
        seamless
        title={
          <div className={styles.headerTitle}>
            {/* Subagent identity only. The scope *switcher* that used to sit
                here is gone — its full family list lives in the ☰ menu, and on a
                390px header the trigger cost 83px to say "主进程" (Main Process)
                about the scope you were already looking at. A subagent still says
                so here (the ↑来自 (from) breadcrumb below names the parent); a main
                session shows nothing, which is where the title needs the width. */}
            {session.isSubagent && (
              <span className={styles.subagentBadge}>⎇ {session.agentType || t("子代理")}</span>
            )}
            {/* The title is the tap target for the session detail sheet. The
                ellipsis hides what the sheet shows (full title, workspace, model,
                various ids). It used to expand an inline panel, whose height had to
                borrow from the body (fitting only five static fields); the sheet
                borrows the whole screen, so watch, subagent, and plan progress
                finally have room to breathe. */}
            <button
              type="button"
              className={styles.titleTap}
              aria-haspopup="dialog"
              aria-expanded={sheetOpen}
              onClick={() => setSheetOpen(true)}
            >
              <span className={styles.headerTitleText}>
                {session.titleOverride || session.aiTitle || session.slug || t("会话")}
              </span>
            </button>
          </div>
        }
        actions={
          /* The pulse status dot used to live in the top-right; it moved into the
             "Running" pill on the status rail. A pulsing 8px unlabeled dot requires
             guessing; two characters make it discoverable. Here we keep just the
             sheet-open button, which does the same as tapping the title, making the
             feature discoverable. */
          <button
            type="button"
            className={styles.moreButton}
            aria-label={t("会话详情")}
            aria-haspopup="dialog"
            aria-expanded={sheetOpen}
            onClick={() => setSheetOpen(true)}
          >
            <MoreHorizontal size={19} />
          </button>
        }
      />

      <StatusRail pills={statusPills} onOpen={openTarget} />

      {sheetOpen && (
        <SessionSheet
          session={session}
          family={family}
          pendingDecisions={pendingDecisions}
          client={client}
          onClose={() => setSheetOpen(false)}
          onOpenPane={setPane}
          onOpenSession={(s) => onOpenSessionId(s.id)}
        />
      )}

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

      {/* The active pane — it overlays the full page (rather than sharing a tab
          bar with messages), so it gets the full screen width and height. The
          Token table and Workflow tree both need horizontal space that the old
          tab layout couldn't spare. */}
      {pane !== null && (
        <div className={styles.pane}>
          <HistoryLayer onBack={() => setPane(null)} />
          <AppHeader onBack={() => setPane(null)} title={t(PANE_TITLE[pane])} />
          <div className={styles.paneScroll}>
            {pane === "decisions" && <DecisionHistoryTab session={session} client={client} />}
            {pane === "plans" && <TaskPlansTab session={session} client={client} />}
            {pane === "token" && <TokenTab session={session} client={client} />}
            {pane === "workflow" && <WorkflowTab session={session} client={client} />}
            {pane === "notes" && <NotesTab session={session} client={client} />}
            {pane === "handoff" && <HandoffTab session={session} client={client} />}
          </div>
        </div>
      )}

      <div
        className={styles.scroll}
        ref={scrollRef}
        onScroll={onScroll}
        style={composerHeight ? { paddingBottom: composerHeight + 14 } : undefined}
      >
        {syncingLatest && messages !== null && (
          <div className={styles.syncingLatest} role="status" aria-live="polite">
            <LoaderCircle size={14} aria-hidden="true" />
            <span>{t("正在同步最新消息…")}</span>
          </div>
        )}
        {messages === null && !loadError && (
          <div className={styles.messageLoading} role="status" aria-live="polite">
            <LoaderCircle size={18} aria-hidden="true" />
            <span>{t("正在同步最新消息…")}</span>
          </div>
        )}
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
              // trailing run opens itself, and shimmers its headline while the
              // session is working.
              const tail = unitIdx === units.length - 1;
              const live = working && tail;
              return (
                <WorkRunBand
                  key={rowKeyOf(unit.msgs[0], unit.startLocal)}
                  msgs={unit.msgs}
                  baseIndex={unit.startLocal}
                  expandedThinking={expandedThinking}
                  onToggleThinking={toggleThinking}
                  live={live}
                  tail={tail}
                  toolMeta={toolMetaMap}
                  resultIds={resultIds}
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
                <div key={rowKeyOf(unit.msgs[0], unit.startLocal)} className={styles.assistantRow}>
                  <MetaFoldCard segments={segments} />
                  <div className={styles.rowTime}>{fmtTime(last.timestamp)}</div>
                </div>
              );
            }
            return (
              <MessageRow
                key={rowKeyOf(unit.msg, unit.startLocal)}
                msg={unit.msg}
                rowKey={rowKeyOf(unit.msg, unit.startLocal)}
                expandedThinking={expandedThinking}
                onToggleThinking={toggleThinking}
                toolMeta={metaForMsg(unit.msg)}
                turnUsage={turnUsage.get(unit.startLocal)}
                client={client}
                jsonlPath={detailPath}
                session={session}
              />
            );
          });
        })()}
        {shownLiveThinking && (
          <div className={styles.liveThinking}>
            <div className={styles.liveThinkingHead}>
              <span className={styles.livePulse} />
              <Sparkles size={13} />
              {t("正在思考…")}
            </div>
            <div className={styles.liveThinkingBody}>{shownLiveThinking.thinking}</div>
          </div>
        )}
        {messages !== null && mainRows.length === 0 && !shownLiveThinking && (
          <EmptyState compact icon={MessageSquareDashed} title={t("暂无可显示的消息")} />
        )}
      </div>

      {canResumeSession(session) && (
        <ResumeComposer
          session={session}
          client={client}
          onOptimisticSend={handleOptimisticSend}
          onSubmitInFlight={handleSubmitInFlight}
          onHeight={setComposerHeight}
        />
      )}
      {canEnqueueSession(session) && (
        <ResumeComposer
          session={session}
          client={client}
          mode="enqueue"
          onSubmitInFlight={handleSubmitInFlight}
          onHeight={setComposerHeight}
        />
      )}
    </div>
    </InFlightToolsContext.Provider>
    </AgentNavProvider>
  );
}
