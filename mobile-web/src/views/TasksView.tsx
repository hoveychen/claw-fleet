import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  CheckCheck,
  CheckCircle2,
  ChevronRight,
  Circle,
  Clock,
  Folder,
  MonitorSmartphone,
  Inbox,
  Loader2,
  CreditCard,
  Radar,
  Search,
  SearchX,
  FileWarning,
  ServerOff,
  Share2,
  Square,
  WifiOff,
} from "lucide-react";
import { AgentSourceIcon } from "./AgentSourceIcon";
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { SessionInfo, SessionMark, SessionStatus } from "../types";
import { isFleetOwnedTask } from "../types";
import { useDraft } from "../draft";
import { itemKey, type WithDevice } from "../deviceRuntime";
import { useChatWorkspaces } from "../useChatWorkspace";
import { useRelaySearch } from "../useRelaySearch";
import { useConfirm } from "../confirmDialog";
import { canControl, runStop, stopMode } from "./sessionStop";
import { repoRootPath } from "../../../shared-ts/repoPath";
import { createQuietLatch, stickyQuiet } from "../../../shared-ts/quietLatch";
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

/** 滚动停下后顺序继续冻结多久。滚动本身也在冻结区间内（每个 scroll 事件都会把
 *  这个计时重新推后），所以实际含义是「滚动期间 + 停手后 5 秒」。 */
const ORDER_FREEZE_MS = 5000;

/**
 * 冻结期内保持列表顺序不变。
 *
 * 任务栏按 lastActivityMs 降序排，而桌面端每隔几秒推一次全量快照：手指还在列表
 * 上滑的时候一次重排，会把手指底下那张卡换成另一张，抬手点下去开的就是别的会话。
 * `frozen` 是冻结那一刻屏幕上的键序（`itemKey`），传 null 表示没冻结、原样透传。
 *
 * 冻结后才出现的会话追加在**末尾**而不是插回它本该在的位置——它按活跃时间本该
 * 排第一，插进去会把每一张卡都顶下一格，正是要避免的那种位移。冻结键序里已经
 * 消失的会话（被筛掉/被清理）直接跳过。
 */
export function applyFrozenOrder<T extends SessionInfo & { deviceId?: string }>(
  rows: T[],
  frozen: readonly string[] | null,
): T[] {
  if (!frozen || frozen.length === 0) return rows;
  const byKey = new Map<string, T>();
  for (const s of rows) byKey.set(itemKey(s.deviceId ?? "", s.id), s);
  const out: T[] = [];
  const taken = new Set<string>();
  for (const key of frozen) {
    const s = byKey.get(key);
    if (!s) continue; // 这条已经不在列表里了
    out.push(s);
    taken.add(key);
  }
  for (const s of rows) {
    if (!taken.has(itemKey(s.deviceId ?? "", s.id))) out.push(s);
  }
  return out;
}

const WORKING: SessionStatus[] = ["thinking", "executing", "streaming", "processing", "delegating"];
const LIVE: SessionStatus[] = [...WORKING, "waitingInput", "active", "rateLimited", "serverErrored", "remoteDisconnected"];

/** Dot tone for the row status accent — mirrors the desktop launchpad's
 *  `rowBarColor`: a colour only for live/waiting rows, `null` (no dot) for
 *  idle. The status is read off the dot, so there's no separate text pill.
 *
 *  `"quiet"` is the third state (desktop `isQuietAlive`): the CLI process is
 *  still running while the scan-computed status has aged out to idle, because
 *  `determine_status` derives status from transcript age alone and a session
 *  parked on one long tool call stops writing. Those rows must not read as
 *  ended — the detail composer offers to *queue* a follow-up for exactly this
 *  session, and the two surfaces must agree.
 *
 *  The quiet tone is *latched* (`shared-ts/quietLatch.ts`, shared with the
 *  desktop row): without hysteresis the same session alternated working ↔ quiet
 *  several times a minute, because it writes one line every few minutes and
 *  each write pushes the status back to a live one for core's hard window. */
const quietLatch = createQuietLatch();

export function statusTone(s: SessionInfo & { deviceId?: string }): string | null {
  const quiet = stickyQuiet(quietLatch, `${s.deviceId ?? ""}/${s.id}`, {
    alive: !!s.procAlive,
    rawQuiet: !!s.procAlive && !LIVE.includes(s.status),
    lastActivityMs: s.lastActivityMs ?? 0,
    now: Date.now(),
  });
  if (s.status === "waitingInput") return "waiting";
  if (s.status === "rateLimited" || s.status === "serverErrored" || s.status === "remoteDisconnected")
    return "error";
  // A latched session stays dim even while its status momentarily reads live —
  // that is the whole point of the hysteresis.
  if (quiet) return "quiet";
  if (WORKING.includes(s.status)) return "working";
  if (s.status === "active") return "active";
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

export function timeAgo(ms: number): string {
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

type MarkFilter = "all" | "pending" | "done";

/** Same bucketing as the desktop launchpad: an unmarked session still needs
 *  attention, so it collapses into "pending" — only an explicit done leaves. */
function markBucket(s: SessionInfo): SessionMark {
  return s.userMark === "done" ? "done" : "pending";
}

/** 任务列表的一个文件夹分区 —— 与桌面端启动台的仓库分组同构。 */
export interface TaskSection {
  /** 分区键,同时也是目录下拉的选项值(`workspaceFilterValue` 编码)。 */
  key: string;
  /** 表头文案:多设备时前缀设备名。 */
  name: string;
  /** 仓库根路径(worktree 已折回)。 */
  path: string;
  deviceId: string;
  sessions: Array<WithDevice<SessionInfo>>;
}

/**
 * 把已排好序的会话切成文件夹分区。分区内**不重排**,保持传入顺序 —— 上面那套
 * 冻结顺序的用心在这里必须原样守住。分区之间按名字字母序(桌面端
 * `groupSessionsByWorkspace` 同款):文件夹是稳定的目录清单,始终在同一个位置,
 * 只有文件夹里的任务随活跃时间浮动。字母序与活跃度无关,所以不会破坏冻结。
 *
 * 纯聊天工作区恒定置顶(桌面端 `groupSessionsByWorkspace` 的 `pinnedPath` 同款):
 * 它是最常回去的一个,不该因为某个项目更活跃就沉到列表深处。多设备时每台机器的
 * 聊天目录各成一个分区,一并提到前面。
 */
export function groupTaskSections(
  rows: Array<WithDevice<SessionInfo>>,
  opts: {
    /** 某台设备的聊天目录，null = 还不知道。按设备问，因为远端主机的聊天目录是
     *  它自己 home 下的路径，与本机那一条不同。 */
    chatPathOf: (deviceId: string) => string | null;
    multiDevice: boolean;
    deviceLabelOf?: (deviceId: string) => string | null | undefined;
  },
): TaskSection[] {
  const { chatPathOf, multiDevice, deviceLabelOf } = opts;
  const byKey = new Map<string, TaskSection>();
  for (const s of rows) {
    const path = repoRootPath(s.workspacePath);
    const key = workspaceFilterValue(s.deviceId, path, multiDevice);
    const existing = byKey.get(key);
    if (existing) {
      existing.sessions.push(s);
      continue;
    }
    const device = deviceLabelOf?.(s.deviceId);
    byKey.set(key, {
      key,
      name: device ? `${device} · ${s.workspaceName}` : s.workspaceName,
      path,
      deviceId: s.deviceId,
      sessions: [s],
    });
  }
  const sections = [...byKey.values()].sort(
    (a, b) => a.name.localeCompare(b.name) || a.key.localeCompare(b.key),
  );
  const isChat = (sec: TaskSection) => sec.path === chatPathOf(sec.deviceId);
  const chat = sections.filter(isChat);
  if (chat.length === 0) return sections;
  return [...chat, ...sections.filter((sec) => !isChat(sec))];
}

/** 目录筛选项的值。单设备时就是路径本身(与从前一致,老的草稿值继续有效)。 */
export function workspaceFilterValue(
  deviceId: string | undefined,
  workspacePath: string,
  multiDevice: boolean,
): string {
  return multiDevice && deviceId ? `${deviceId}::${workspacePath}` : workspacePath;
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
function chainTip<T extends SessionInfo>(members: T[]): T {
  return members.reduce((a, b) => ((b.handoff?.hop ?? 0) > (a.handoff?.hop ?? 0) ? b : a));
}

/** One entry in the rendered task list: either a standalone session or a
 *  collapsed handoff-relay chain. */
// 泛型是为了让 deviceId 一路带到渲染:调用方传进来的是 WithDevice<SessionInfo>,
// 折叠成接力组之后每一项仍然要知道自己属于哪一台设备。
type RenderItem<T extends SessionInfo = SessionInfo> =
  | { kind: "single"; key: string; session: T }
  | {
      kind: "group";
      key: string;
      chainId: string;
      chainLen: number;
      tip: T;
      members: T[];
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
export function buildRenderItems<T extends SessionInfo & { deviceId?: string }>(
  rows: T[],
  group: boolean,
): Array<RenderItem<T>> {
  // 合并列表里 id 只在单机内唯一,所以分组键与 React key 都带上归属设备。
  // 不带的话两台机器上碰巧同 chainId 的接力链会被折进同一组,展开后是一串
  // 属于不同机器的会话 —— 点进去就是拿错设备的 transport 去拉一条它不认识的
  // 会话。
  const scope = (s: T) => s.deviceId ?? "";
  if (!group)
    return rows.map((s) => ({ kind: "single", key: `${scope(s)}::${s.id}`, session: s }));
  const items: Array<RenderItem<T>> = [];
  const groupAt = new Map<string, number>();
  for (const s of rows) {
    const cid = s.handoff && s.handoff.chainLen > 1 ? s.handoff.chainId : null;
    if (!cid) {
      items.push({ kind: "single", key: `${scope(s)}::${s.id}`, session: s });
      continue;
    }
    const groupKey = `${scope(s)}::${cid}`;
    const at = groupAt.get(groupKey);
    if (at === undefined) {
      groupAt.set(groupKey, items.length);
      items.push({
        kind: "group",
        key: `chain:${groupKey}`,
        chainId: cid,
        chainLen: s.handoff!.chainLen,
        tip: s,
        members: [s],
      });
    } else {
      (items[at] as Extract<RenderItem<T>, { kind: "group" }>).members.push(s);
    }
  }
  return items.map((it) => {
    if (it.kind !== "group") return it;
    if (it.members.length < 2) {
      return {
        kind: "single",
        key: `${scope(it.members[0])}::${it.members[0].id}`,
        session: it.members[0],
      };
    }
    const members = [...it.members].sort((a, b) => b.handoff!.hop - a.handoff!.hop);
    return { ...it, tip: chainTip(members), members };
  });
}

interface Props {
  /** 合并列表:每条会话都带着它属于哪一台设备(deviceRuntime.ts 的 WithDevice)。
   *  id 只在单机内唯一,所以 React key 与「打开这一条」都必须带上 deviceId。 */
  sessions: Array<WithDevice<SessionInfo>>;
  /** 某一条会话所属设备的传输层。任务页**不持有**当前作用域那一台的 client:
   *  列表是合并的,所以每一次**写**(标记/中断/停止)以及每一次按会话取数(搜索、
   *  聊天目录)都必须按设备取,否则请求会打到当前选中的那台机器上:标记落在别的
   *  主机 → 下一次快照推回来 userMark 还是空 → 卡片复活;停止更糟,pid 会被发到
   *  一台毫不相干的主机上执行。 */
  clientFor: (deviceId: string) => FleetTransport | null;
  /** WS link phone↔relay. */
  connected: boolean;
  /** Desktop↔relay link — false means nothing is there to push a snapshot. */
  agentOnline: boolean;
  /** Whether at least one `sessions` snapshot has arrived since connecting.
   *  Distinguishes "still waiting for the first push" from "pushed, but empty". */
  sessionsLoaded: boolean;
  onOpenSession: (session: WithDevice<SessionInfo>) => void;
  /** 这台设备的显示名。整个 prop 缺席 = 只配了一台,徽标与「设备 · 目录」的
   *  筛选项都不出现 —— 单设备用户不该为多设备付出任何一处视觉噪音。 */
  deviceLabelOf?: (deviceId: string) => string | null;
}

// 新会话入口由 App 底部导航中间的凸起按钮统一持有，任务页内不再重复放置。
export function TasksView({
  sessions,
  clientFor,
  deviceLabelOf,
  connected,
  agentOnline,
  sessionsLoaded,
  onOpenSession,
}: Props) {
  const confirm = useConfirm();
  // 筛选状态落到 localStorage（复用 Composer 草稿那套 useDraft），这样切标签页
  // 卸载重挂、乃至 iOS 杀掉 PWA 后再回来，搜索词/目录/分段都保持不变，
  // 不会每次回任务页都被复位。busyOp / markOverride 是瞬时态，仍走普通 useState。
  const [search, setSearch] = useDraft<string>("tasks:search", "");
  const [markFilter, setMarkFilter] = useDraft<MarkFilter>("tasks:markFilter", "all");
  // Group handoff-relay chains into one collapsible card. Default on; the setter
  // lives in the More tab. Tabs unmount on switch, so this re-reads the saved
  // value whenever the task page remounts — no cross-tab live sync needed.
  const [groupHandoff] = useDraft<boolean>("tasks:groupHandoff", true);
  const [busyOp, setBusyOp] = useState<string | null>(null);
  // Optimistic mark overrides, dropped once the server snapshot catches up.
  const [markOverride, setMarkOverride] = useState<Record<string, SessionMark | null>>({});

  /** 列表里出现过的设备。搜索与聊天目录都是**逐台**问的。 */
  const deviceIds = useMemo(() => [...new Set(sessions.map((s) => s.deviceId))], [sessions]);

  // Full-text search over the relay — same FTS the desktop launchpad uses,
  // 每台设备各问一次自己的索引。
  const { searching, ftsMatchKeys, snippetByKey } = useRelaySearch(deviceIds, clientFor, search);

  // 滚动期间（及停手后 ORDER_FREEZE_MS 内）冻住的键序，null = 未冻结。
  // 状态用于让下面的 useMemo 重算；ref 是滚动回调里的唯一真相（回调闭包读不到
  // 最新 state，而这里必须在同一批 scroll 事件里立刻知道「已经冻上了」）。
  const [frozenOrder, setFrozenOrder] = useState<string[] | null>(null);
  const frozenRef = useRef<string[] | null>(null);
  /** 最近一次渲染出的键序 —— 冻结时按它取样，也就是用户此刻真正看到的顺序。 */
  const renderedOrderRef = useRef<string[]>([]);

  // Scope to Fleet-launched sessions (新会话 / handoff relay), exactly like the
  // desktop 任务 launchpad (`adhocSessions`). Externally-started transcripts
  // (VS Code / bare CLI) are deliberately out of this list.
  const all = useMemo(
    () =>
      applyFrozenOrder(
        sessions
          .filter(isFleetOwnedTask)
          .map((s) => {
            const o = markOverride[itemKey(s.deviceId, s.id)];
            return o !== undefined && o !== (s.userMark ?? null) ? { ...s, userMark: o } : s;
          })
          .sort((a, b) => b.lastActivityMs - a.lastActivityMs),
        frozenOrder,
      ),
    [sessions, markOverride, frozenOrder],
  );

  useEffect(() => {
    renderedOrderRef.current = all.map((s) => itemKey(s.deviceId, s.id));
  }, [all]);

  // The desktop host's pure-chat workspace — the same path the new-session sheet
  // pins. Null while it's in flight; the chat section then simply sits where its
  // activity puts it instead of being pinned on a guess.
  const chatPaths = useChatWorkspaces(deviceIds, clientFor);
  const chatPathOf = useCallback(
    (deviceId: string) => chatPaths[deviceId] ?? null,
    [chatPaths],
  );

  // 列表按文件夹分区展示（Chat 置顶），所以任务页不再有目录下拉：要看哪个目录
  // 就折叠掉别的分区。「终端」按钮因此不带初始目录，由终端页自己的目录选择器接手。
  const multiDevice = deviceLabelOf !== undefined;

  // Everything except the mark filter — the segment counts are taken over this
  // set so each count reflects how many rows its segment would reveal under the
  // current query (mirrors the desktop `preMark`).
  const preMark = useMemo(() => {
    const q = search.trim().toLowerCase();
    return all.filter((s) => {
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
        if (!clientMatch && !ftsMatchKeys.has(itemKey(s.deviceId, s.jsonlPath))) return false;
      }
      return true;
    });
  }, [all, search, ftsMatchKeys]);

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

  const setMark = useCallback(
    (s: WithDevice<SessionInfo>, mark: SessionMark | null) => {
      // 会话所属那一台,不是当前作用域那一台 —— 打错主机的标记等于没标记。
      const transport = clientFor(s.deviceId);
      if (!transport) return;
      const key = itemKey(s.deviceId, s.id);
      setMarkOverride((prev) => ({ ...prev, [key]: mark }));
      transport
        .request("session_mark", {
          sessionId: s.id,
          workspacePath: s.workspacePath,
          ...(mark ? { mark } : {}),
        })
        .catch(() => {
          // roll back the optimistic flip on failure
          setMarkOverride((prev) => {
            const next = { ...prev };
            delete next[key];
            return next;
          });
        });
    },
    [clientFor],
  );

  // Full membership of every relay chain, keyed by chainId — over ALL Fleet
  // sessions (not the filtered `visible`), so a group's mark-all covers the
  // whole chain even when the mark filter hides some hops, and the header's
  // aggregate done-state reflects the entire chain. `all` already folds in the
  // optimistic `markOverride`, so this reacts on tap.
  const chainMembersAll = useMemo(() => {
    const m = new Map<string, Array<WithDevice<SessionInfo>>>();
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
    (members: Array<WithDevice<SessionInfo>>, done: boolean) => {
      for (const m of members) setMark(m, done ? "done" : null);
    },
    [setMark],
  );

  // 文件夹分区是列表的一级层次；接力链分组退到分区之内（与桌面端启动台同构），
  // 所以 buildRenderItems 逐分区跑，不会把两个目录的会话串成一条链。
  const sections = useMemo(
    () =>
      groupTaskSections(visible, { chatPathOf, multiDevice, deviceLabelOf }).map((sec) => ({
        ...sec,
        items: buildRenderItems(sec.sessions, groupHandoff),
      })),
    [visible, chatPathOf, multiDevice, deviceLabelOf, groupHandoff],
  );

  // 折叠起来的分区键。默认全展开——手机上一进来就该看到会话本身。
  const [collapsedSections, setCollapsedSections] = useState<Set<string>>(() => new Set());
  const toggleSection = useCallback((key: string) => {
    setCollapsedSections((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);

  const handleStop = useCallback(
    async (s: WithDevice<SessionInfo>) => {
      // pid / workspacePath 只在**它自己那台主机**上有意义:发到别的设备上,轻则
      // 停不掉,重则按 pid 打到一个毫不相干的进程。
      const transport = clientFor(s.deviceId);
      if (!transport || busyOp) return;
      setBusyOp(itemKey(s.deviceId, s.id));
      try {
        await runStop(transport, s, confirm);
      } catch (e) {
        window.alert(e instanceof Error ? e.message : t("操作失败"));
      } finally {
        setBusyOp(null);
      }
    },
    [clientFor, busyOp, confirm],
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
  //
  // 同一个监听里还负责冻结排序：第一个 scroll 事件把当前键序拍下来，之后每个
  // 事件把解冻计时推后，停手 ORDER_FREEZE_MS 后才放开重排。
  useEffect(() => {
    let idle: ReturnType<typeof setTimeout> | undefined;
    let thaw: ReturnType<typeof setTimeout> | undefined;
    const onScroll = () => {
      if (idle) clearTimeout(idle);
      idle = setTimeout(() => {
        if (maxScroll() > 4) savedTasksScrollY = window.scrollY;
      }, 150);
      if (frozenRef.current == null) {
        const snapshot = renderedOrderRef.current;
        frozenRef.current = snapshot;
        setFrozenOrder(snapshot);
      }
      if (thaw) clearTimeout(thaw);
      thaw = setTimeout(() => {
        frozenRef.current = null;
        setFrozenOrder(null);
      }, ORDER_FREEZE_MS);
    };
    window.addEventListener("scroll", onScroll, { passive: true });
    return () => {
      window.removeEventListener("scroll", onScroll);
      if (idle) clearTimeout(idle);
      if (thaw) clearTimeout(thaw);
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
    s: WithDevice<SessionInfo>,
    group?: {
      expanded: boolean;
      onToggleExpand: () => void;
      /** Whole-chain membership the mark toggle fans out across. */
      markMembers: Array<WithDevice<SessionInfo>>;
    },
  ) => {
    // For a collapsed group the header card is the tip, but its dot must
    // reflect the whole chain (`group.markMembers` = full membership), not just
    // the tip — otherwise a chain floated to the top by a live mid-hop shows no
    // dot. Plain cards keep deriving from the session itself.
    const tone = group ? chainTone(group.markMembers) : statusTone(s);
    const mode = stopMode(s);
    const isDone = group
      ? group.markMembers.length > 0 && group.markMembers.every((m) => m.userMark === "done")
      : s.userMark === "done";
    const title =
      s.titleOverride || s.aiTitle || s.slug || s.lastMessagePreview || t("（无标题）");
    const snippet =
      search.trim().length >= 2 ? snippetByKey.get(itemKey(s.deviceId, s.jsonlPath)) : undefined;
    const live = LIVE.includes(s.status);
    return (
      <div
        key={itemKey(s.deviceId, s.id)}
        className={styles.card}
        onClick={() => onOpenSession(s)}
      >
        <div className={styles.cardHead}>
          {tone && <span className={styles.statusDot} data-tone={tone} />}
          <span className={styles.sourceIcon} title={s.agentSource || "claude-code"}>
            <AgentSourceIcon source={s.agentSource} />
          </span>
          <span className={styles.title}>{title}</span>
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
          {/* 目录名不再逐行重复——它就写在这张卡所属分区的表头上。 */}
          {/* 合并列表里必须一眼看出这条会话在哪台机器上 —— 同名项目在两台机器
              上很常见,而点进去拉的是那一台的 transcript。 */}
          {deviceLabelOf?.(s.deviceId) && (
            <span className={styles.device}>
              <MonitorSmartphone size={11} />
              {deviceLabelOf(s.deviceId)}
            </span>
          )}
          {s.handoff && (
            <span className={styles.handoff}>
              <Share2 size={11} />
              {s.handoff.hop}/{s.handoff.chainLen}
            </span>
          )}
          {/* 远端 ssh 隧道断了、会话被 Fleet 停掉 —— 手机上只看到一个红点会
              以为是普通报错,必须把「哪台机器没了」直接写出来。 */}
          {s.remoteDisconnect && (
            <span
              className={styles.remoteLost}
              title={s.remoteDisconnect.detail}
            >
              <ServerOff size={11} />
              {s.remoteDisconnect.agentStopped
                ? t("{0} 断开,已停止", s.remoteDisconnect.hostLabel ?? t("远端"))
                : t("{0} 断开,agent 未停", s.remoteDisconnect.hostLabel ?? t("远端"))}
            </span>
          )}
          {/* 输出落在了错的机器上 —— 会话本身跑得好好的,不提就没人会发现。 */}
          {s.mirrorWrite && (
            <span
              className={styles.remoteLost}
              title={t(
                "这些文件留在了本机镜像目录 {0},没同步到远端主机:{1}",
                s.mirrorWrite.workspacePath,
                s.mirrorWrite.files.join(", "),
              )}
            >
              <FileWarning size={11} />
              {t("{0} 个文件留在本机", String(s.mirrorWrite.total))}
            </span>
          )}
          {/* 账号额度耗尽 —— 它没有 status(也没有 reset 时刻,等的是有人去充值),
              所以这行在手机上看起来和正常跑完一模一样,只有这枚标签会说。实心,
              和上面两枚描边的区分开:这个状态自己不会好。 */}
          {s.outOfCredits && (
            <span className={styles.outOfCredits} title={s.outOfCredits}>
              <CreditCard size={11} />
              {t("额度耗尽")}
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
              disabled={busyOp === itemKey(s.deviceId, s.id)}
              onClick={() => void handleStop(s)}
            >
              {busyOp === itemKey(s.deviceId, s.id) ? (
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
        {sections.map((section) => {
          const collapsed = collapsedSections.has(section.key);
          return (
            <section key={section.key} className={styles.workspaceSection}>
              <button
                className={styles.workspaceHeader}
                aria-expanded={!collapsed}
                title={section.path}
                onClick={() => toggleSection(section.key)}
              >
                <Folder size={13} className={styles.workspaceFolder} />
                <span className={styles.workspaceName}>{section.name}</span>
                {/* 折叠后的组数：一条折叠的接力链算一组，与桌面端二级侧栏同义。 */}
                <span className={styles.workspaceCount}>{section.items.length}</span>
                <ChevronRight
                  size={14}
                  className={styles.workspaceChevron}
                  data-open={!collapsed}
                />
              </button>
              {!collapsed &&
                section.items.map((item) => {
                  if (item.kind === "single") return renderCard(item.session);
                  const { chainId, tip, members, key } = item;
                  const full = chainMembersAll.get(chainId) ?? members;
                  const expanded = expandedChains.has(chainId);
                  const limit = chainLoadMore[chainId] ?? GROUP_VISIBLE;
                  // The header card *is* the tip (latest hop); the expanded list
                  // shows only the chain's *other* hops, never the tip again.
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
            </section>
          );
        })}
      </div>

    </div>
  );
}
