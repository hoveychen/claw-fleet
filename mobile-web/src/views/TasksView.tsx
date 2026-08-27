import { useCallback, useEffect, useLayoutEffect, useMemo, useState, type ReactNode } from "react";
import {
  CheckCheck,
  CheckCircle2,
  ChevronRight,
  Circle,
  Clock,
  Folder,
  Inbox,
  Loader2,
  Radar,
  Search,
  SearchX,
  Share2,
  Square,
  WifiOff,
} from "lucide-react";
import { AgentSourceIcon } from "./AgentSourceIcon";
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { SessionInfo, SessionMark, SessionStatus } from "../types";
import { isFleetOwnedEntrypoint, isFleetOwnedTask, isSessionUnread } from "../types";
import { useDraft } from "../draft";
import { useChatWorkspace } from "../useChatWorkspace";
import { useRelaySearch } from "../useRelaySearch";
import styles from "./TasksView.module.css";

/** 文档级滚动条被所有 tab 共享，任务页又会随 tab 卸载重挂（见 App 里按 `tab` 的条件
 *  渲染），加上 iOS PWA 前后台切换、重连时推来的全量快照，都可能把 window.scrollY
 *  打回 0。这里记住用户停留的位置，重挂或意外回顶后恢复它，且绝不和正在滚动的用户较劲。
 *  位置存模块级变量：切 tab 重挂时 JS 未重载、位置还在；整页刷新时自然归零——一次全新
 *  加载理应从顶端开始。 */
let savedTasksScrollY = 0;

/** 页面当前的最大可滚动距离；<= 4px 视作「短到不必滚动」，此时既不记录也不恢复。 */
function maxScroll(): number {
  return document.documentElement.scrollHeight - window.innerHeight;
}

const WORKING: SessionStatus[] = ["thinking", "executing", "streaming", "processing", "delegating"];
const LIVE: SessionStatus[] = [...WORKING, "waitingInput", "active", "rateLimited", "serverErrored"];

/** Dot tone for the row status accent — mirrors the desktop launchpad's
 *  `rowBarColor`: a colour only for live/waiting rows, `null` (no dot) for
 *  idle. The status is read off the dot, so there's no separate text pill.
 *
 *  `"quiet"` is the third state (desktop `isQuietAlive`): the CLI process is
 *  still running while the scan-computed status has aged out to idle, because
 *  `determine_status` derives status from transcript age alone and a session
 *  parked on one long tool call stops writing. Those rows must not read as
 *  ended — the detail composer offers to *queue* a follow-up for exactly this
 *  session, and the two surfaces must agree. */
export function statusTone(s: SessionInfo): string | null {
  if (WORKING.includes(s.status)) return "working";
  if (s.status === "waitingInput") return "waiting";
  if (s.status === "active") return "active";
  if (s.status === "rateLimited" || s.status === "serverErrored") return "error";
  if (s.procAlive) return "quiet";
  return null;
}

/** Highest-salience status tone across a collapsed relay group's members. The
 *  header card is the tip, but the group floats up the list on its most recently
 *  active member (which need not be the tip), so its dot must reflect the whole
 *  chain — a running hop outranks a waiting/active/errored one, mirroring the
 *  desktop launchpad's `chainBarColor`. */
const TONE_PRIORITY = ["working", "waiting", "active", "error", "quiet"];
function chainTone(members: SessionInfo[]): string | null {
  let best: string | null = null;
  let bestRank = TONE_PRIORITY.length;
  for (const m of members) {
    const tn = statusTone(m);
    if (tn == null) continue;
    const r = TONE_PRIORITY.indexOf(tn);
    if (r >= 0 && r < bestRank) {
      bestRank = r;
      best = tn;
      if (r === 0) break; // a running hop wins outright
    }
  }
  return best;
}

function timeAgo(ms: number): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return t("刚刚");
  if (diff < 3_600_000) return t("{0} 分钟前", Math.floor(diff / 60_000));
  if (diff < 86_400_000) return t("{0} 小时前", Math.floor(diff / 3_600_000));
  return t("{0} 天前", Math.floor(diff / 86_400_000));
}

/** Elapsed run time = now − session start; shown only for live rows, matching
 *  the desktop launchpad's `formatRunning`. Single-unit, counts up from start. */
function formatRunning(ms: number): string {
  const diff = Math.max(0, Date.now() - ms);
  if (diff < 60_000) return t("运行 {0} 秒", Math.floor(diff / 1_000));
  if (diff < 3_600_000) return t("运行 {0} 分", Math.floor(diff / 60_000));
  if (diff < 86_400_000) return t("运行 {0} 时", Math.floor(diff / 3_600_000));
  return t("运行 {0} 天", Math.floor(diff / 86_400_000));
}

/** Elapsed since a watch was registered, single-unit and counting up — mirrors
 *  formatRunning but with "已过" (waited) wording for the watch chip. */
function formatWatchElapsed(ms: number): string {
  const diff = Math.max(0, Date.now() - ms);
  if (diff < 60_000) return t("已过 {0} 秒", Math.floor(diff / 1_000));
  if (diff < 3_600_000) return t("已过 {0} 分", Math.floor(diff / 60_000));
  if (diff < 86_400_000) return t("已过 {0} 时", Math.floor(diff / 3_600_000));
  return t("已过 {0} 天", Math.floor(diff / 86_400_000));
}

type StopMode = "interrupt" | "stop" | "spent";

/** Same escalation as the desktop StopControl: interrupt only for Fleet-owned
 *  sessions with a precise pid mid-turn; otherwise kill; no pid → dead. */
function stopMode(s: SessionInfo): StopMode {
  if (s.pid == null) return "spent";
  if (isFleetOwnedEntrypoint(s.entrypoint) && s.pidPrecise && WORKING.includes(s.status)) {
    return "interrupt";
  }
  return "stop";
}

function canControl(s: SessionInfo): boolean {
  return !s.isSubagent;
}

type MarkFilter = "all" | "pending" | "done";

/** Same bucketing as the desktop launchpad: an unmarked session still needs
 *  attention, so it collapses into "pending" — only an explicit done leaves. */
function markBucket(s: SessionInfo): SessionMark {
  return s.userMark === "done" ? "done" : "pending";
}

/** Values the workspace filter used to take back when the pure-chat workspace
 *  was still an option inside the `<select>` rather than its own toggle. "" is
 *  "all" and every real value is an absolute path, so these bare words could
 *  never collide with one. Read-only: migrated away on the first render that
 *  finds one persisted (see the effect in TasksView). */
const LEGACY_CHAT_ONLY = "chat";
const LEGACY_CHAT_HIDDEN = "no-chat";

/** Does `s` pass the workspace filter? Mirrors the desktop launchpad's
 *  `matchesWorkspaceFilter`: `chatOnly` is a chat-*only* toggle, not a
 *  chat-on/chat-off switch. On, it shows the pure-chat workspace and nothing
 *  else (the directory filter is ignored underneath it); off, nothing is
 *  filtered by mode, so chat sessions sit in "all" alongside the repos.
 *
 *  `chatPath` is the desktop host's `~/.fleet/chat`, `null` until the relay
 *  answers (or if it never does) — the toggle cannot be honoured without it, so
 *  it goes inert rather than showing an empty list. */
export function matchesWorkspaceFilter(
  s: SessionInfo,
  filter: string,
  chatPath: string | null,
  chatOnly: boolean,
): boolean {
  if (chatPath != null && chatOnly) return s.workspacePath === chatPath;
  if (!filter) return true;
  return s.workspacePath === filter;
}

/** FTS5 snippets arrive with literal `<mark>…</mark>` markers (see the desktop
 *  HistoryView). Split them into React nodes instead of trusting the transcript
 *  text as HTML. */
function renderSnippet(snippet: string): ReactNode[] {
  return snippet.split("<mark>").flatMap((chunk, i) => {
    if (i === 0) return [chunk];
    const end = chunk.indexOf("</mark>");
    if (end === -1) return [chunk];
    return [<mark key={i}>{chunk.slice(0, end)}</mark>, chunk.slice(end + "</mark>".length)];
  });
}

/** How many chain members an expanded group shows before "load more"; a relay
 *  chain can run 50 hops deep, so we reveal the most recent few and page in the
 *  rest on demand. Mirrors the desktop launchpad's HistoryView. */
const GROUP_VISIBLE = 3;
const GROUP_LOAD_STEP = 10;

/** Highest-hop (tip / latest relay) member — the chain's "current" session. */
function chainTip(members: SessionInfo[]): SessionInfo {
  return members.reduce((a, b) => ((b.handoff?.hop ?? 0) > (a.handoff?.hop ?? 0) ? b : a));
}

/** One entry in the rendered task list: either a standalone session or a
 *  collapsed handoff-relay chain. */
type RenderItem =
  | { kind: "single"; key: string; session: SessionInfo }
  | {
      kind: "group";
      key: string;
      chainId: string;
      chainLen: number;
      tip: SessionInfo;
      members: SessionInfo[];
    };

/**
 * Fold a flat, already-filtered+sorted list into render items, collapsing
 * sessions that share a `handoff.chainId` into one group. A chain becomes a
 * group only when ≥2 of its members are present; a lone surviving hop renders
 * as an ordinary card (keeping its own handoff chip). The group takes the list
 * position of its first (most recently active) member; members are ordered
 * newest-hop-first so "show last N" reveals the recent relays first. Same shape
 * as the desktop launchpad's `buildRenderItems`.
 */
export function buildRenderItems(rows: SessionInfo[], group: boolean): RenderItem[] {
  if (!group) return rows.map((s) => ({ kind: "single", key: s.id, session: s }));
  const items: RenderItem[] = [];
  const groupAt = new Map<string, number>();
  for (const s of rows) {
    const cid = s.handoff && s.handoff.chainLen > 1 ? s.handoff.chainId : null;
    if (!cid) {
      items.push({ kind: "single", key: s.id, session: s });
      continue;
    }
    const at = groupAt.get(cid);
    if (at === undefined) {
      groupAt.set(cid, items.length);
      items.push({
        kind: "group",
        key: `chain:${cid}`,
        chainId: cid,
        chainLen: s.handoff!.chainLen,
        tip: s,
        members: [s],
      });
    } else {
      (items[at] as Extract<RenderItem, { kind: "group" }>).members.push(s);
    }
  }
  return items.map((it) => {
    if (it.kind !== "group") return it;
    if (it.members.length < 2) {
      return { kind: "single", key: it.members[0].id, session: it.members[0] };
    }
    const members = [...it.members].sort((a, b) => b.handoff!.hop - a.handoff!.hop);
    return { ...it, tip: chainTip(members), members };
  });
}

/**
 * When a collapsed relay-group header is opened, which of its members to mark
 * read immediately. The header aggregates its unread dot over the whole chain
 * (`markMembers.some(isSessionUnread)`), but opening it only navigates to the
 * tip's detail — where the existing dwell clears just the tip. The *other* hops
 * of a collapsed group never get a detail dwell of their own, so without this
 * they'd keep the group's dot lit forever after the user has plainly opened it.
 * The tip is excluded so its own 2s detail dwell still governs it (a quick
 * glance that backs out shouldn't clear the tip). Already-read members are
 * skipped so we don't re-stamp them.
 */
export function groupOpenReadTargets(
  tip: SessionInfo,
  markMembers: SessionInfo[],
): SessionInfo[] {
  return markMembers.filter((m) => m.id !== tip.id && isSessionUnread(m));
}

interface Props {
  sessions: SessionInfo[];
  client: FleetTransport | null;
  /** WS link phone↔relay. */
  connected: boolean;
  /** Desktop↔relay link — false means nothing is there to push a snapshot. */
  agentOnline: boolean;
  /** Whether at least one `sessions` snapshot has arrived since connecting.
   *  Distinguishes "still waiting for the first push" from "pushed, but empty". */
  sessionsLoaded: boolean;
  onOpenSession: (session: SessionInfo) => void;
  onMarkRead: (sessions: SessionInfo[]) => void;
}

// 新会话入口由 App 底部导航中间的凸起按钮统一持有，任务页内不再重复放置。
export function TasksView({
  sessions,
  client,
  connected,
  agentOnline,
  sessionsLoaded,
  onOpenSession,
  onMarkRead,
}: Props) {
  // 筛选状态落到 localStorage（复用 Composer 草稿那套 useDraft），这样切标签页
  // 卸载重挂、乃至 iOS 杀掉 PWA 后再回来，搜索词/目录/仅活跃/分段都保持不变，
  // 不会每次回任务页都被复位。busyOp / markOverride 是瞬时态，仍走普通 useState。
  const [search, setSearch] = useDraft<string>("tasks:search", "");
  const [workspace, setWorkspace] = useDraft<string>("tasks:workspace", "");
  // 仅聊天模式 —— 打开时盖过上面的目录筛选；关闭时不按模式过滤，聊天会话照常
  // 混在列表里（见 matchesWorkspaceFilter）。
  const [chatOnly, setChatOnly] = useDraft<boolean>("tasks:chatOnly", false);
  const [activeOnly, setActiveOnly] = useDraft<boolean>("tasks:activeOnly", false);
  const [markFilter, setMarkFilter] = useDraft<MarkFilter>("tasks:markFilter", "all");
  // Group handoff-relay chains into one collapsible card. Default on; the setter
  // lives in the More tab. Tabs unmount on switch, so this re-reads the saved
  // value whenever the task page remounts — no cross-tab live sync needed.
  const [groupHandoff] = useDraft<boolean>("tasks:groupHandoff", true);
  const [busyOp, setBusyOp] = useState<string | null>(null);
  // Optimistic mark overrides, dropped once the server snapshot catches up.
  const [markOverride, setMarkOverride] = useState<Record<string, SessionMark | null>>({});

  // Full-text search over the relay — same FTS the desktop launchpad uses.
  const { searching, ftsMatchPaths, snippetByPath } = useRelaySearch(client, search);

  // Scope to Fleet-launched sessions (新会话 / handoff relay), exactly like the
  // desktop 任务 launchpad (`adhocSessions`). Externally-started transcripts
  // (VS Code / bare CLI) are deliberately out of this list.
  const all = useMemo(
    () =>
      sessions
        .filter(isFleetOwnedTask)
        .map((s) => {
          const o = markOverride[s.id];
          return o !== undefined && o !== (s.userMark ?? null) ? { ...s, userMark: o } : s;
        })
        .sort((a, b) => b.lastActivityMs - a.lastActivityMs),
    [sessions, markOverride],
  );

  // The desktop host's pure-chat workspace — the same path the new-session sheet
  // pins. Null while it's in flight, which keeps the chat options hidden rather
  // than offering a filter that can't be honoured.
  const chatPath = useChatWorkspace(client);

  // Chat is filtered through its own pinned options, so it is kept out of the
  // project list — otherwise it would sit there a second time as plain "Chat".
  const workspaces = useMemo(() => {
    const names = new Map<string, string>();
    for (const s of all) {
      if (s.workspacePath === chatPath) continue;
      names.set(s.workspacePath, s.workspaceName);
    }
    return [...names.entries()].sort((a, b) => a[1].localeCompare(b[1]));
  }, [all, chatPath]);

  // 迁移：聊天还是下拉里一条选项时存下来的值。"chat" 交给 toggle，"no-chat"
  // 已经没有对应项了（「全部目录」现在也含聊天），直接归零。要抢在下面那个孤
  // 儿路径回退之前跑，否则「只看聊天」会被静默还原成「看全部」。
  useEffect(() => {
    if (workspace === LEGACY_CHAT_ONLY) {
      setWorkspace("");
      setChatOnly(true);
    } else if (workspace === LEGACY_CHAT_HIDDEN) {
      setWorkspace("");
    }
  }, [workspace, setWorkspace, setChatOnly]);

  // A persisted workspace path can outlive its sessions (all done + pruned, or a
  // repo we haven't touched this launch). Left as-is it would silently filter the
  // list to empty while the <select> falls back to showing "全部目录" — looks like
  // a bug. Once the first snapshot has landed, drop an orphaned real path back to
  // "all". Chat mode is unaffected: it lives in its own toggle.
  useEffect(() => {
    if (!sessionsLoaded) return;
    if (!workspace || workspace === LEGACY_CHAT_ONLY || workspace === LEGACY_CHAT_HIDDEN) return;
    if (workspaces.some(([path]) => path === workspace)) return;
    setWorkspace("");
  }, [sessionsLoaded, workspace, workspaces, setWorkspace]);

  const activeCount = useMemo(() => all.filter((s) => LIVE.includes(s.status)).length, [all]);

  // Everything except the mark filter — the segment counts are taken over this
  // set so each count reflects how many rows its segment would reveal under the
  // current workspace / query / active filters (mirrors the desktop `preMark`).
  const preMark = useMemo(() => {
    const q = search.trim().toLowerCase();
    return all.filter((s) => {
      if (!matchesWorkspaceFilter(s, workspace, chatPath, chatOnly)) return false;
      if (activeOnly && !LIVE.includes(s.status)) return false;
      if (q) {
        const clientMatch =
          `${s.titleOverride ?? ""} ${s.aiTitle ?? ""} ${s.slug ?? ""} ${s.lastMessagePreview ?? ""} ${s.workspaceName}`
            .toLowerCase()
            .includes(q) ||
          // The attributed TASKS.md plan — id / title / current P-task, so a
          // handoff relay chain is reachable by the plan name too.
          (s.taskPlan?.planId?.toLowerCase().includes(q) ?? false) ||
          (s.taskPlan?.currentPlan?.toLowerCase().includes(q) ?? false) ||
          (s.taskPlan?.currentTask?.toLowerCase().includes(q) ?? false);
        if (!clientMatch && !ftsMatchPaths.has(s.jsonlPath)) return false;
      }
      return true;
    });
  }, [all, search, workspace, chatPath, chatOnly, activeOnly, ftsMatchPaths]);

  const counts = useMemo(() => {
    let pending = 0;
    let done = 0;
    for (const s of preMark) {
      if (markBucket(s) === "done") done++;
      else pending++;
    }
    return { all: preMark.length, pending, done };
  }, [preMark]);

  const visible = useMemo(
    () => preMark.filter((s) => markFilter === "all" || markBucket(s) === markFilter),
    [preMark, markFilter],
  );

  const unreadCount = useMemo(() => visible.filter(isSessionUnread).length, [visible]);

  const setMark = useCallback(
    (s: SessionInfo, mark: SessionMark | null) => {
      if (!client) return;
      setMarkOverride((prev) => ({ ...prev, [s.id]: mark }));
      client
        .request("session_mark", {
          sessionId: s.id,
          workspacePath: s.workspacePath,
          ...(mark ? { mark } : {}),
        })
        .catch(() => {
          // roll back the optimistic flip on failure
          setMarkOverride((prev) => {
            const next = { ...prev };
            delete next[s.id];
            return next;
          });
        });
    },
    [client],
  );

  // Full membership of every relay chain, keyed by chainId — over ALL Fleet
  // sessions (not the filtered `visible`), so a group's mark-all covers the
  // whole chain even when the mark filter hides some hops, and the header's
  // aggregate done-state reflects the entire chain. `all` already folds in the
  // optimistic `markOverride`, so this reacts on tap.
  const chainMembersAll = useMemo(() => {
    const m = new Map<string, SessionInfo[]>();
    for (const s of all) {
      if (s.handoff && s.handoff.chainLen > 1) {
        const arr = m.get(s.handoff.chainId);
        if (arr) arr.push(s);
        else m.set(s.handoff.chainId, [s]);
      }
    }
    return m;
  }, [all]);

  const [expandedChains, setExpandedChains] = useState<Set<string>>(() => new Set());
  const [chainLoadMore, setChainLoadMore] = useState<Record<string, number>>({});
  const toggleChain = useCallback((cid: string) => {
    setExpandedChains((prev) => {
      const next = new Set(prev);
      if (next.has(cid)) next.delete(cid);
      else next.add(cid);
      return next;
    });
  }, []);
  const loadMoreChain = useCallback((cid: string) => {
    setChainLoadMore((prev) => ({
      ...prev,
      [cid]: (prev[cid] ?? GROUP_VISIBLE) + GROUP_LOAD_STEP,
    }));
  }, []);
  const setMarkChain = useCallback(
    (members: SessionInfo[], done: boolean) => {
      for (const m of members) setMark(m, done ? "done" : null);
    },
    [setMark],
  );

  const renderItems = useMemo(
    () => buildRenderItems(visible, groupHandoff),
    [visible, groupHandoff],
  );

  const handleStop = useCallback(
    async (s: SessionInfo) => {
      if (!client || busyOp) return;
      const mode = stopMode(s);
      if (mode === "spent") return;
      setBusyOp(s.id);
      try {
        if (mode === "interrupt") {
          await client.request("interrupt", { pid: s.pid });
        } else if (s.pidPrecise) {
          if (!window.confirm(t("确定停止「{0}」的这个会话吗？", s.workspaceName))) return;
          await client.request("stop", { pid: s.pid });
        } else {
          if (
            !window.confirm(
              t("无法精确定位进程，将停止「{0}」目录下的所有会话，确定吗？", s.workspaceName),
            )
          )
            return;
          await client.request("stop_workspace", { workspacePath: s.workspacePath });
        }
      } catch (e) {
        window.alert(e instanceof Error ? e.message : t("操作失败"));
      } finally {
        setBusyOp(null);
      }
    },
    [client, busyOp],
  );

  // ——— 滚动位置稳定化（详见文件顶部 savedTasksScrollY 的注释）———
  const restore = useCallback(() => {
    // 只在「已经意外回到顶部、但记忆位置在下方、且页面够长能容纳」时纠回，这样
    // 主动滚到顶的用户不会被硬拽下去，内容没坍缩的正常刷新（scrollY 不为 0）也不受扰。
    if (window.scrollY < 4 && savedTasksScrollY > 4 && maxScroll() >= savedTasksScrollY - 4) {
      window.scrollTo(0, savedTasksScrollY);
    }
  }, []);

  // 滚动停歇 150ms 后才记录「用户真正停留的位置」。若某个外部事件把 scrollY 瞬间
  // 夹到 0，下面的恢复会在停歇前把它纠回，所以那个瞬时 0 永远不会被记下来。
  useEffect(() => {
    let idle: ReturnType<typeof setTimeout> | undefined;
    const onScroll = () => {
      if (idle) clearTimeout(idle);
      idle = setTimeout(() => {
        if (maxScroll() > 4) savedTasksScrollY = window.scrollY;
      }, 150);
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      if (idle) clearTimeout(idle);
    };
  }, []);

  // 挂载时（切 tab 回到任务页）无条件恢复到上次停留处——此刻 window.scrollY 属于刚
  // 离开的那个 tab，并非任务页意图。列表在 all.length>0 时已同步渲染出完整高度，故
  // useLayoutEffect 里能立即定位、绘制前完成，无闪烁。
  useLayoutEffect(() => {
    if (savedTasksScrollY > 4 && maxScroll() >= savedTasksScrollY - 4) {
      window.scrollTo(0, savedTasksScrollY);
    }
  }, []);

  // 每次数据刷新后，若 scrollY 被外部事件意外打回顶部就纠回（guard 保证只在真回顶时动）。
  useLayoutEffect(() => {
    restore();
  }, [sessions, restore]);

  // PWA 从后台回到前台时同样纠一次——iOS 常在恢复前台时重置文档滚动。
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible") restore();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [restore]);

  // Distinguish the reasons the list can be empty, so a blank screen never
  // leaves 老板 guessing "是错了还是在加载". Order matters: connectivity first
  // (can we even get data?), then whether the first snapshot has landed, then
  // a genuine "no tasks". Only the last is a true empty; the others are
  // transient/among-actionable states with their own copy and a spinner.
  if (all.length === 0) {
    if (!connected) {
      return (
        <EmptyState
          spin
          icon={Loader2}
          title={t("正在连接…")}
          description={t("正在连接中转服务。若长时间停在这里，请检查手机网络。")}
        />
      );
    }
    if (!agentOnline) {
      return (
        <EmptyState
          icon={WifiOff}
          title={t("桌面端离线")}
          description={t("已连上中转，但桌面端 Fleet 未在线，暂时拿不到任务快照。请确认电脑上的 Fleet 正在运行。")}
        />
      );
    }
    if (!sessionsLoaded) {
      return (
        <EmptyState
          spin
          icon={Loader2}
          title={t("正在加载任务…")}
          description={t("桌面端在线，正在接收首屏快照，通常一两秒内到达。")}
        />
      );
    }
    return (
      <EmptyState
        icon={Inbox}
        title={t("还没有会话")}
        description={t("桌面端各会话上线后会出现在这里。")}
      />
    );
  }

  const segLabels: Record<MarkFilter, string> = {
    all: t("全部"),
    pending: t("进行中"),
    done: t("已完成"),
  };

  // One session card — shared by standalone cards, the tip of a relay group
  // (with `group` set: adds an expand chevron + whole-chain mark), and the
  // members inside an expanded group, so all three stay identical.
  const renderCard = (
    s: SessionInfo,
    group?: {
      expanded: boolean;
      onToggleExpand: () => void;
      /** Whole-chain membership the mark toggle fans out across. */
      markMembers: SessionInfo[];
    },
  ) => {
    // For a collapsed group the header card is the tip, but its dot and unread
    // bold must reflect the whole chain (`group.markMembers` = full membership),
    // not just the tip — otherwise a chain floated to the top by a live mid-hop
    // shows no dot. Plain cards keep deriving from the session itself.
    const tone = group ? chainTone(group.markMembers) : statusTone(s);
    const mode = stopMode(s);
    const unread = group ? group.markMembers.some(isSessionUnread) : isSessionUnread(s);
    const isDone = group
      ? group.markMembers.length > 0 && group.markMembers.every((m) => m.userMark === "done")
      : s.userMark === "done";
    const title =
      s.titleOverride || s.aiTitle || s.slug || s.lastMessagePreview || t("（无标题）");
    const snippet = search.trim().length >= 2 ? snippetByPath.get(s.jsonlPath) : undefined;
    const live = LIVE.includes(s.status);
    return (
      <div
        key={s.id}
        className={styles.card}
        onClick={() => {
          onOpenSession(s);
          // A group header aggregates unread over the whole chain, but opening
          // it only dwells the tip — clear the other unread hops here so the
          // group's dot doesn't linger after the user opened it.
          if (group) {
            const rest = groupOpenReadTargets(s, group.markMembers);
            if (rest.length > 0) onMarkRead(rest);
          }
        }}
      >
        <div className={styles.cardHead}>
          {tone && <span className={styles.statusDot} data-tone={tone} />}
          <span className={styles.sourceIcon} title={s.agentSource || "claude-code"}>
            <AgentSourceIcon source={s.agentSource} />
          </span>
          <span className={styles.title}>{title}</span>
          {unread && <span className={styles.unreadDot} />}
          <span className={styles.time}>{timeAgo(s.lastActivityMs)}</span>
          {group && (
            <button
              className={styles.groupToggle}
              data-open={group.expanded}
              aria-expanded={group.expanded}
              onClick={(e) => {
                e.stopPropagation();
                group.onToggleExpand();
              }}
              title={group.expanded ? t("收起接力链") : t("展开接力链上更早的会话")}
            >
              <ChevronRight size={16} className={styles.groupChevron} data-open={group.expanded} />
            </button>
          )}
        </div>
        <div className={styles.metaRow}>
          <span className={styles.project}>
            <Folder size={11} />
            {s.workspaceName}
          </span>
          {s.handoff && (
            <span className={styles.handoff}>
              <Share2 size={11} />
              {s.handoff.hop}/{s.handoff.chainLen}
            </span>
          )}
          {s.watches?.map((w) => (
            <span
              key={w.id}
              className={styles.handoff}
              title={w.note ?? undefined}
            >
              <Radar size={11} />
              {formatWatchElapsed(w.created)} · {t("轮询 {0} 次", w.pollCount)}
            </span>
          ))}
          {(live || tone === "quiet") && (
            <span className={styles.runtime} data-tone={tone ?? undefined}>
              <Clock size={11} />
              {formatRunning(s.createdAtMs)}
            </span>
          )}
        </div>
        {snippet ? (
          <div className={styles.snippet}>{renderSnippet(snippet)}</div>
        ) : (
          s.lastMessagePreview && <div className={styles.preview}>{s.lastMessagePreview}</div>
        )}
        <div className={styles.opsRow} onClick={(e) => e.stopPropagation()}>
          <button
            className={styles.markToggle}
            data-done={isDone}
            onClick={() =>
              group
                ? setMarkChain(group.markMembers, !isDone)
                : setMark(s, isDone ? null : "done")
            }
            aria-pressed={isDone}
            title={
              group
                ? isDone
                  ? t("整条接力链已完成 — 点击全部改回进行中")
                  : t("点击把整条接力链标为已完成")
                : isDone
                  ? t("已完成 — 点击改回进行中")
                  : t("进行中 — 点击标为已完成")
            }
          >
            {group ? (
              <CheckCheck size={16} opacity={isDone ? 1 : 0.55} />
            ) : isDone ? (
              <CheckCircle2 size={16} />
            ) : (
              <Circle size={15} />
            )}
          </button>
          <span className={styles.opsSpacer} />
          {canControl(s) && mode !== "spent" && (
            <button
              className={styles.stopButton}
              data-mode={mode}
              disabled={busyOp === s.id}
              onClick={() => void handleStop(s)}
            >
              {busyOp === s.id ? (
                "…"
              ) : (
                <>
                  <Square size={12} />
                  {mode === "interrupt" ? t("中断") : t("停止")}
                </>
              )}
            </button>
          )}
        </div>
      </div>
    );
  };

  return (
    <div className={styles.wrapper}>
      <div className={styles.filterBar}>
        <div className={styles.searchWrap}>
          <span className={styles.searchIcon}>
            <Search size={14} />
          </span>
          <input
            className={styles.search}
            type="search"
            placeholder={t("搜索标题、计划、全文…")}
            value={search}
            onChange={(e) => setSearch(e.target.value)}
          />
          {searching && <span className={styles.searchSpinner} />}
        </div>
        <div className={styles.filterRow}>
          {/* 目录下拉和聊天开关是同一个筛选的两半，互斥：聊天模式占满整个
              列表，所以下拉在它底下置灰失效，而不是偷偷收窄一个它已经管不着
              的列表。relay 没报出聊天目录时整个开关不显示——那时它没法生效。*/}
          <select
            className={styles.workspaceSelect}
            value={workspace}
            onChange={(e) => setWorkspace(e.target.value)}
            disabled={chatOnly}
          >
            <option value="">{t("全部目录")}</option>
            {workspaces.map(([path, name]) => (
              <option key={path} value={path}>
                {name}
              </option>
            ))}
          </select>
          {chatPath && (
            <button
              className={styles.filterToggle}
              data-active={chatOnly}
              onClick={() => setChatOnly((v) => !v)}
            >
              💬 {t("聊天")}
            </button>
          )}
          <button
            className={styles.filterToggle}
            data-active={activeOnly}
            onClick={() => setActiveOnly((v) => !v)}
          >
            <span className={styles.activeDot} />
            {t("仅活跃")}
            <span className={styles.activeCount}>{activeCount}</span>
          </button>
          {unreadCount > 0 && (
            <button
              className={styles.readAll}
              onClick={() => onMarkRead(visible.filter(isSessionUnread))}
            >
              {t("全部已读 ({0})", unreadCount)}
            </button>
          )}
        </div>
        <div className={styles.segment}>
          {(["all", "pending", "done"] as MarkFilter[]).map((key) => (
            <button
              key={key}
              className={styles.segmentButton}
              data-active={markFilter === key}
              onClick={() => setMarkFilter(key)}
              aria-label={`${segLabels[key]} (${counts[key]})`}
              title={segLabels[key]}
            >
              {key === "all" && <span>{segLabels.all}</span>}
              {key === "pending" && <Circle size={15} />}
              {key === "done" && <CheckCircle2 size={16} />}
              <span className={styles.segmentCount}>{counts[key]}</span>
            </button>
          ))}
        </div>
      </div>

      {!sessionsLoaded && all.length > 0 && (
        <div className={styles.syncingHint}>
          <Loader2 size={12} className={styles.syncingSpin} />
          {t("显示上次缓存，正在同步…")}
        </div>
      )}

      {visible.length === 0 && (
        <EmptyState compact icon={SearchX} title={t("没有匹配的会话")} />
      )}

      <div className={styles.list}>
        {renderItems.map((item) => {
          if (item.kind === "single") return renderCard(item.session);
          const { chainId, tip, members, key } = item;
          const full = chainMembersAll.get(chainId) ?? members;
          const expanded = expandedChains.has(chainId);
          const limit = chainLoadMore[chainId] ?? GROUP_VISIBLE;
          // The header card *is* the tip (latest hop); the expanded list shows
          // only the chain's *other* hops, never the tip again.
          const rest = members.filter((m) => m.id !== tip.id);
          const shown = expanded ? rest.slice(0, limit) : [];
          const hidden = rest.length - shown.length;
          return (
            <div key={key} className={styles.group}>
              {renderCard(tip, {
                expanded,
                onToggleExpand: () => toggleChain(chainId),
                markMembers: full,
              })}
              {expanded && (
                <div className={styles.groupChildren}>
                  {shown.map((m) => renderCard(m))}
                  {hidden > 0 && (
                    <button
                      className={styles.groupMore}
                      onClick={() => loadMoreChain(chainId)}
                    >
                      {t("显示更早的 {0} 棒", hidden)}
                    </button>
                  )}
                </div>
              )}
            </div>
          );
        })}
      </div>

    </div>
  );
}
