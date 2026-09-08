import { memo, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import { Bot, ChevronRight, Clock, FolderGit2, MessageCircleQuestion, Radar, Waypoints } from "lucide-react";
import type { SessionInfo } from "../types";
import { LIVE_STATUSES, isQuietAlive, isQuietAliveSticky, rowBarColor } from "../types";
import { pendingDecisionState } from "../pendingDecisionState";
import { useDecisionStore } from "../store";
import { MarkControl } from "./MarkControl";
import { AgentSourceIcon } from "./SessionCard";
import styles from "./SessionRow.module.css";

export function timeAgo(ms: number, t: (k: string, opts?: Record<string, unknown>) => string): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return t("just_now");
  if (diff < 3_600_000) return t("m_ago", { n: Math.floor(diff / 60_000) });
  if (diff < 86_400_000) return t("h_ago", { n: Math.floor(diff / 3_600_000) });
  return t("d_ago", { n: Math.floor(diff / 86_400_000) });
}

/** Elapsed run time = now − session start (createdAtMs). Shown only for live
 *  rows (busy or waiting-input), where "how long has this been going" is the
 *  useful signal. Same single-unit shape as timeAgo, but counts up from start
 *  instead of down from last activity. */
function formatRunning(ms: number, t: (k: string, opts?: Record<string, unknown>) => string): string {
  const diff = Math.max(0, Date.now() - ms);
  if (diff < 60_000) return t("ran_s", { n: Math.floor(diff / 1_000) });
  if (diff < 3_600_000) return t("ran_m", { n: Math.floor(diff / 60_000) });
  if (diff < 86_400_000) return t("ran_h", { n: Math.floor(diff / 3_600_000) });
  return t("ran_d", { n: Math.floor(diff / 86_400_000) });
}

/** Compact, language-neutral elapsed ("40s" / "3m" / "2h" / "1d") for the tight
 *  row watch chip — mirrors SessionCard's formatDuration unit style. */
function compactElapsed(ms: number): string {
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h`;
  return `${Math.floor(h / 24)}d`;
}

/** FTS5 snippets arrive with literal `<mark>…</mark>` markers (see
 *  search_index.rs). Split them into React nodes instead of trusting the
 *  transcript text as HTML. */
function renderSnippet(snippet: string): ReactNode[] {
  return snippet.split("<mark>").flatMap((chunk, i) => {
    if (i === 0) return [chunk];
    const end = chunk.indexOf("</mark>");
    if (end === -1) return [chunk];
    return [
      <mark key={i}>{chunk.slice(0, end)}</mark>,
      chunk.slice(end + "</mark>".length),
    ];
  });
}

/**
 * Compare two SessionInfo by value. The scanner re-emits the whole list every
 * couple of seconds and each entry is a fresh JSON deserialisation, so object
 * identity always differs even when nothing changed — a reference check would
 * make SessionRow's memo useless.
 *
 * Deliberately keyed off the object's own keys rather than a hand-maintained
 * field list: a new SessionInfo field is then covered automatically, instead of
 * silently freezing a row that a forgotten list entry would have updated.
 * Scalars compare with `===`; the few nested fields (handoff, taskPlan, todos)
 * fall back to a structural compare — they are small.
 */
export function sessionEq(a: SessionInfo, b: SessionInfo): boolean {
  if (a === b) return true;
  const keys = Object.keys(a) as (keyof SessionInfo)[];
  if (keys.length !== Object.keys(b).length) return false;
  for (const k of keys) {
    const va = a[k];
    const vb = b[k];
    if (va === vb) continue;
    if (va && vb && typeof va === "object" && typeof vb === "object") {
      if (JSON.stringify(va) !== JSON.stringify(vb)) return false;
      continue;
    }
    return false;
  }
  return true;
}

export type SessionRowProps = {
  session: SessionInfo;
  snippet: string | undefined;
  isSelected: boolean;
  /** Open in a tab, but not the tab currently on screen. Weaker half of the
   *  same axis as `isSelected` — never both at once. */
  isOpen: boolean;
  /** Bumped every 30s by the parent so relative times keep advancing. Without
   *  it the memo would freeze the elapsed "3 分钟" at whatever it said on mount. */
  nowTick: number;
  /** True only when the task page holds more than one agent source at once
   *  (e.g. Claude and Codex): the little source glyph then rides ahead of the
   *  title so the two are told apart at a glance. Hidden when everything is the
   *  same source — a badge that never varies is just noise. */
  showSource: boolean;
  /** Hidden when a repository heading already labels this row. */
  showWorkspace?: boolean;
  onClick: (s: SessionInfo) => void;
  onContextMenu: (e: React.MouseEvent, s: SessionInfo) => void;
  /** Overrides the run-status dot colour. A collapsed relay group's header is
   *  the tip session, but its dot must reflect the *whole chain's* liveness (see
   *  `chainBarColor`), so the header passes the aggregate here instead of letting
   *  the row derive it from the tip alone. `undefined` = derive from the session
   *  (the normal single-row path); `null` = force no dot. */
  runColorOverride?: string | null;
  /** Relay-chain group header mode: when set, this row is a chain's tip session
   *  and gains an expand/collapse chevron (toggling the chain's other hops) plus
   *  a supplied whole-chain mark control in place of the single-row one. Clicking
   *  the row body still opens the tip like any other session. */
  expandable?: {
    expanded: boolean;
    onToggle: () => void;
    /** Whole-chain mark toggle, rendered instead of the single-session one. */
    markControl: React.ReactNode;
  };
};

export const SessionRow = memo(function SessionRow({
  session: s,
  snippet,
  isSelected,
  isOpen,
  showSource,
  showWorkspace = true,
  onClick,
  onContextMenu,
  runColorOverride,
  expandable,
}: SessionRowProps) {
  const { t } = useTranslation();
  const runColor = runColorOverride !== undefined ? runColorOverride : rowBarColor(s);
  // Process alive, transcript quiet (see `isQuietAlive`): the row keeps its dot
  // — faded, so it reads apart from a truly working session — and the runtime
  // chip stays up, since "how long has this been going" is exactly the question
  // a session stuck on one long tool call raises.
  // Raw vs latched (see `isQuietAliveSticky`): the dot's colour follows the
  // latched value so it stops flickering, so the tooltip has to as well — a
  // faded dot reading "运行中" would contradict itself. The two differ only in
  // the window right after a sparse write, which gets its own wording: the
  // transcript just moved, but the cadence is still "one line every few
  // minutes", which is what the faded dot is saying.
  // Does this task have a card waiting on 老板? Simplified mode has no
  // always-on DecisionPanel, so without this chip a card raised on a task you
  // are not currently reading is invisible — and a *parked* (timed-out) one
  // stays invisible for as long as it takes to open that task by hand.
  // A store subscription, not a prop: the memo below compares props only, and
  // the selector returns a plain string so a re-render happens exactly when
  // this row's state changes.
  const decisionState = useDecisionStore((st) => pendingDecisionState(st.decisions, s.id));
  const quiet = isQuietAlive(s);
  const sparse = !quiet && isQuietAliveSticky(s);
  const quietMins = quiet
    ? Math.max(0, Math.round((Date.now() - s.lastActivityMs) / 60000))
    : 0;
  // The title renders on a single clamped line, so the tooltip is the only
  // place the full text survives — carry the title *and* the last message,
  // not just the message (which is what a truncated row leaves you guessing).
  const displayTitle =
    s.titleOverride ?? s.aiTitle ?? s.slug ?? s.lastMessagePreview ?? t("history.untitled", "（无标题）");
  const tooltip = [displayTitle, s.lastMessagePreview]
    .filter((v): v is string => !!v && v.trim().length > 0)
    .filter((v, i, all) => all.indexOf(v) === i)
    .join("\n\n");
  // Gray subtitle line, same idea as the card view's `.preview`: the last
  // message at a glance, without hovering. Suppressed when it would just
  // repeat the title, and when a search snippet already fills that slot.
  const preview =
    !snippet && s.lastMessagePreview && s.lastMessagePreview !== displayTitle
      ? s.lastMessagePreview
      : null;
  return (
    <div
      className={styles.row_wrap}
      onContextMenu={(e) => onContextMenu(e, s)}
    >
      <button
        type="button"
        className={`${styles.row} ${expandable ? styles.row_expandable : ""} ${isSelected ? styles.row_active : isOpen ? styles.row_open : ""}`}
        onClick={() => onClick(s)}
        title={tooltip || undefined}
      >
        {runColor && (
          <span
            className={styles.row_status_dot}
            style={{ background: runColor }}
            title={
              quiet
                ? t("history.quiet_alive", "运行中 · 已 {{mins}} 分钟没有新输出（多半卡在一条长工具调用上）", {
                    mins: quietMins,
                  })
                : s.status === "waitingInput"
                  ? t("history.waiting", "等待输入")
                  : sparse
                    ? t("history.quiet_sparse", "运行中 · 输出稀疏（每隔几分钟才写一行，多半卡在一条长工具调用上）")
                    : t("history.running", "运行中")
            }
          />
        )}
        <span className={styles.row_body}>
          <span className={styles.row_title}>
            {showSource && (
              <span className={styles.row_source} title={s.agentSource}>
                <AgentSourceIcon source={s.agentSource} />
              </span>
            )}
            {displayTitle}
          </span>
          {preview && <span className={styles.row_preview}>{preview}</span>}
          <span className={styles.row_meta}>
            {showWorkspace && (
              <span className={styles.row_project} title={s.workspacePath}>
                <FolderGit2 size={10} strokeWidth={1.6} />
                {s.workspaceName}
              </span>
            )}
            {decisionState !== "none" && (
              <span
                className={
                  decisionState === "parked" ? styles.row_decision_parked : styles.row_decision
                }
                title={
                  decisionState === "parked"
                    ? t(
                        "history.decision_parked",
                        "决策卡已超时 — 会话被暂停，问题还在等你回复",
                      )
                    : t("history.decision_pending", "有决策卡在等你回复")
                }
              >
                <MessageCircleQuestion size={10} strokeWidth={1.6} />
                {decisionState === "parked"
                  ? t("history.decision_parked_chip", "已超时")
                  : t("history.decision_pending_chip", "待决策")}
              </span>
            )}
            {s.handoff && (
              <span
                className={styles.row_handoff}
                title={t("card.tip_handoff", {
                  hop: s.handoff.hop,
                  len: s.handoff.chainLen,
                })}
              >
                <Waypoints size={10} strokeWidth={1.6} />
                {s.handoff.hop}/{s.handoff.chainLen}
              </span>
            )}
            {s.watches?.map((w) => (
              <span
                key={w.id}
                className={styles.row_handoff}
                title={[
                  w.note ?? undefined,
                  t("card.tip_watch", {
                    elapsed: compactElapsed(w.created),
                    count: w.pollCount,
                    poll: w.pollSecs,
                  }),
                ]
                  .filter(Boolean)
                  .join(" — ")}
              >
                <Radar size={10} strokeWidth={1.6} />
                {compactElapsed(w.created)}·{w.pollCount}
              </span>
            ))}
            {(LIVE_STATUSES.has(s.status) || quiet) && (
              <span
                className={styles.row_runtime}
                style={{ color: runColor ?? undefined }}
                title={new Date(s.createdAtMs).toLocaleString()}
              >
                <Clock size={10} strokeWidth={1.6} />
                {formatRunning(s.createdAtMs, t)}
              </span>
            )}
            {s.runningSubagentCount > 0 && (
              <span
                className={styles.row_subagents}
                style={{ color: runColor ?? undefined }}
                title={t("history.running_subagents", {
                  count: s.runningSubagentCount,
                })}
              >
                <Bot size={10} strokeWidth={1.6} />
                {s.runningSubagentCount}
              </span>
            )}
            {/* Activity time uses the aggregate (own ∪ subagents): a session
                delegating to subagents keeps a stale own `lastActivityMs` while
                the tree churns, so this reflects the whole tree being alive. */}
            <span
              className={styles.row_time}
              title={
                s.agentLastActivityMs !== s.lastActivityMs
                  ? t("history.activity_via_subagent")
                  : undefined
              }
            >
              {timeAgo(s.agentLastActivityMs, t)}
            </span>
          </span>
          {snippet && <span className={styles.row_snippet}>{renderSnippet(snippet)}</span>}
        </span>
      </button>
      <span className={styles.row_actions}>
        {expandable && (
          <button
            type="button"
            className={styles.group_expand}
            onClick={(e) => {
              e.stopPropagation();
              expandable.onToggle();
            }}
            aria-expanded={expandable.expanded}
            title={
              expandable.expanded
                ? t("history.group_collapse", "收起接力链")
                : t("history.group_expand", "展开接力链上更早的会话")
            }
          >
            <ChevronRight
              className={styles.group_chevron}
              data-open={expandable.expanded ? "true" : "false"}
              size={14}
              strokeWidth={2}
            />
          </button>
        )}
        {expandable ? expandable.markControl : <MarkControl session={s} />}
      </span>
    </div>
  );
},
(prev, next) =>
  prev.isSelected === next.isSelected &&
  prev.isOpen === next.isOpen &&
  prev.snippet === next.snippet &&
  prev.nowTick === next.nowTick &&
  prev.showSource === next.showSource &&
  prev.runColorOverride === next.runColorOverride &&
  prev.onClick === next.onClick &&
  prev.onContextMenu === next.onContextMenu &&
  // Group headers pass a fresh markControl element + toggle closure each render,
  // so this never short-circuits them; that's fine (headers are few) and keeps
  // the chevron's open-state in sync. Plain rows leave `expandable` undefined,
  // so they still benefit from the memo.
  prev.expandable === next.expandable &&
  sessionEq(prev.session, next.session));
