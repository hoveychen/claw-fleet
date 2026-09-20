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
  Activity,
  CheckCheck,
  CheckCircle2,
  ChevronRight,
  Circle,
  Clock,
  Folder,
  List,
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
  TimerOff,
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
import { countChainUnits } from "../../../shared-ts/chainUnits";
import { createQuietLatch, stickyQuiet } from "../../../shared-ts/quietLatch";
import { STATUS_BUCKETS, type StatusBucket } from "../../../shared-ts/statusBuckets";
import styles from "./TasksView.module.css";

/** Document-level scrollbar is shared by all tabs; the task view unmounts/remounts with
 *  the tab (see conditional rendering in App), and iOS PWA background/foreground switches
 *  plus reconnection full-state snapshots can reset window.scrollY to 0. This module
 *  remembers user scroll position and restores it on remount or accidental return to top,
 *  without fighting a scrolling user. Position stored in module state: tab switches don't
 *  reload JS so position persists; full page refresh naturally resets to zero, which is
 *  the right behavior. */
let savedTasksScrollY = 0;

/** Maximum scrollable distance on the page; <= 4px is considered "too short to need scrolling",
 *  so position is neither recorded nor restored. */
function maxScroll(): number {
  return document.documentElement.scrollHeight - window.innerHeight;
}

/** How long after scroll stops the sort order stays frozen. Scrolling itself resets this
 *  timer (every scroll event pushes it back), so the real meaning is "during scroll + 5s
 *  after user stops". */
const ORDER_FREEZE_MS = 5000;

/**
 * Preserve list order during the freeze window.
 *
 * The task bar sorts by descending lastActivityMs, and the desktop pushes a full snapshot
 * every few seconds. While fingers are swiping on the list, a reorder swaps the card under
 * the finger for another—releasing and tapping opens the wrong session. `frozen` holds the
 * key sequence (`itemKey`) as it was at freeze time; null means unfrozen, pass-through.
 *
 * Sessions appearing after freeze are appended to the end, not inserted at their sorted
 * position—they'd rank first by activity, shifting every card down, which is exactly the
 * tap-miss we avoid. Sessions that vanished from the frozen key sequence (filtered/cleaned)
 * are skipped.
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
    if (!s) continue; // Already removed from the list
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
  // Checked first: a watch-parked session has no process and writes nothing, so
  // it would otherwise fall through to the final `return null` — no dot, reading
  // as ended — when a Fleet timer is in fact going to resume it.
  if (s.status === "watching") return "watching";
  if (s.status === "waitingInput") return "waiting";
  // `stuck` joins the error family rather than falling through to the quiet
  // branch below: a wedged turn keeps its process alive and writes nothing, so
  // hysteresis would dim it to "quiet" — the one reading that says "nothing to
  // see here" about the one state that always needs a human.
  if (
    s.status === "rateLimited" ||
    s.status === "serverErrored" ||
    s.status === "remoteDisconnected" ||
    s.status === "stuck"
  )
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
const TONE_PRIORITY = ["working", "waiting", "active", "error", "quiet", "watching"];
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

/** One folder partition in the task list, isomorphic to the repository groups on the
 *  desktop launchpad. */
export interface TaskSection {
  /** Partition key, also used as the directory dropdown option value (encoded by
   *  `workspaceFilterValue`). */
  key: string;
  /** Header label: prefixed with device name when multi-device. */
  name: string;
  /** Repository root path (worktrees folded back). */
  path: string;
  deviceId: string;
  sessions: Array<WithDevice<SessionInfo>>;
}

/**
 * Slice a pre-sorted session list into folder partitions. Partitions are **not resorted**
 * internally; input order is preserved—the frozen-order machinery above must stay intact
 * here. Partitions sort alphabetically by name (same logic as desktop `groupSessionsByWorkspace`):
 * folders form a stable directory listing always in the same position, only sessions within
 * a folder float by activity. Alphabetic order is independent of activity, so it won't
 * break the freeze.
 *
 * Pure-chat workspaces always pin to the top (same as desktop `groupSessionsByWorkspace`'s
 * `pinnedPath`): it's the most-revisited one and shouldn't sink deep when another project
 * gets active. Multi-device: each machine's chat directory is its own partition, all pinned
 * to the front.
 */
export function groupTaskSections(
  rows: Array<WithDevice<SessionInfo>>,
  opts: {
    /** Chat directory for a given device; null = not yet determined. Query per-device
     *  because remote hosts have their own chat directory path under their home,
     *  different from this machine's. */
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

/** What the task list's sections stand for — the same three the desktop task
 *  rail offers. "none" drops the headings and lists every card in one stream. */
export type TaskGroupMode = "workspace" | "status" | "none";

/** Which run-status section a card belongs to, derived from the very tone its
 *  dot wears (see `statusTone`) so a heading can never contradict the colour
 *  under it. A null tone is an ended session. */
export function bucketOfTone(tone: string | null): StatusBucket {
  if (tone === "waiting") return "waitingInput";
  if (tone === "watching") return "watching";
  if (tone === "error") return "error";
  if (tone === null) return "ended";
  return "running";
}

/** The dot tone a status heading wears — the tone most of its cards wear, so
 *  the heading reads as a label for the colour under it. The running bucket
 *  spans three tones (working / active / quiet); it takes `working`, the one a
 *  busy session shows, with the pulse suppressed in CSS so a heading does not
 *  throb alongside the live cards. */
const BUCKET_HEADER_TONE: Record<StatusBucket, string> = {
  running: "working",
  waitingInput: "waiting",
  error: "error",
  watching: "watching",
  ended: "idle",
};

/** Heading text for a run-status section. */
function bucketLabel(bucket: StatusBucket): string {
  if (bucket === "running") return t("运行中");
  if (bucket === "waitingInput") return t("等待输入");
  if (bucket === "error") return t("出错/限流");
  if (bucket === "watching") return t("等待触发");
  return t("已结束");
}

/** One run-status partition, shaped like `TaskSection` so the list renders both
 *  groupings through the same code. */
export interface StatusTaskSection {
  bucket: StatusBucket;
  key: string;
  sessions: Array<WithDevice<SessionInfo>>;
}

/**
 * Slice a pre-sorted session list into run-status partitions, in the fixed
 * `STATUS_BUCKETS` order. Input order is preserved inside a partition (the
 * frozen-order machinery must stay intact), and empty buckets are dropped.
 */
export function groupStatusSections(
  rows: Array<WithDevice<SessionInfo>>,
): StatusTaskSection[] {
  const byBucket = new Map<StatusBucket, Array<WithDevice<SessionInfo>>>();
  for (const s of rows) {
    const bucket = bucketOfTone(statusTone(s));
    const arr = byBucket.get(bucket);
    if (arr) arr.push(s);
    else byBucket.set(bucket, [s]);
  }
  return STATUS_BUCKETS.filter((b) => byBucket.get(b)?.length).map((bucket) => ({
    bucket,
    key: `status:${bucket}`,
    sessions: byBucket.get(bucket)!,
  }));
}

/** Value of a directory filter option. Single-device: the path itself (for backward
 *  compatibility, old draft values still work). */
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
// Generic parameter ensures deviceId flows through to render: caller passes
// WithDevice<SessionInfo>, and after folding into relay groups each item still needs
// to know which device it belongs to.
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
  // In merged lists, id is unique only per-machine, so group key and React key both
  // include the owning device. Without it, relay chains with the same chainId on two
  // machines would fold into one group; expanding would show sessions from different
  // machines—opening would use the wrong device's transport to fetch an unknown session.
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
  /** Merged list: each session carries its device (WithDevice from deviceRuntime.ts).
   *  Id is unique only per-machine, so React key and "open this one" must include
   *  deviceId. */
  sessions: Array<WithDevice<SessionInfo>>;
  /** Transport for the device a session belongs to. TasksView does **not** hold the
   *  current scope's client: the list is merged, so every write (mark/interrupt/stop)
   *  and every per-session lookup (search, chat directory) must query per-device, or the
   *  request goes to the currently selected machine. Mark on wrong host → next snapshot
   *  has empty userMark → card comes back; stop is worse, pid gets sent to an unrelated
   *  machine. */
  clientFor: (deviceId: string) => FleetTransport | null;
  /** WS link phone↔relay. */
  connected: boolean;
  /** Desktop↔relay link — false means nothing is there to push a snapshot. */
  agentOnline: boolean;
  /** Whether at least one `sessions` snapshot has arrived since connecting.
   *  Distinguishes "still waiting for the first push" from "pushed, but empty". */
  sessionsLoaded: boolean;
  onOpenSession: (session: WithDevice<SessionInfo>) => void;
  /** Display label for this device. Prop absent = only one configured, so badge and
   *  "device · directory" filter don't appear—single-device users shouldn't pay
   *  any visual noise tax for multi-device. */
  deviceLabelOf?: (deviceId: string) => string | null;
}

// New session entry point is owned by the raised button in the center of App's bottom
// navigation; no duplicate in the task page.
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
  // Filter state persists in localStorage (reusing the Composer draft pattern via useDraft),
  // so tab switches unmount/remount, iOS PWA background/foreground, all preserve search terms,
  // directories, and segments—no reset on every return to the task page. busyOp and
  // markOverride are transient, using plain useState.
  const [search, setSearch] = useDraft<string>("tasks:search", "");
  const [markFilter, setMarkFilter] = useDraft<MarkFilter>("tasks:markFilter", "all");
  // Group handoff-relay chains into one collapsible card. Default on; the setter
  // lives in the More tab. Tabs unmount on switch, so this re-reads the saved
  // value whenever the task page remounts — no cross-tab live sync needed.
  const [groupHandoff] = useDraft<boolean>("tasks:groupHandoff", true);
  // What the list's sections stand for. Persisted like the filters above, for
  // the same reason: every tab switch unmounts this view.
  const [groupMode, setGroupMode] = useDraft<TaskGroupMode>("tasks:groupMode", "workspace");
  const [busyOp, setBusyOp] = useState<string | null>(null);
  // Optimistic mark overrides, dropped once the server snapshot catches up.
  const [markOverride, setMarkOverride] = useState<Record<string, SessionMark | null>>({});

  /** Devices appearing in the list. Search and chat directory both query per-device. */
  const deviceIds = useMemo(() => [...new Set(sessions.map((s) => s.deviceId))], [sessions]);

  // Full-text search over the relay — same FTS the desktop launchpad uses,
  // querying each device's own index.
  const { searching, ftsMatchKeys, snippetByKey } = useRelaySearch(deviceIds, clientFor, search);

  // Key sequence frozen during scroll (and ORDER_FREEZE_MS after), null = unfrozen.
  // State lets useMemos below recalculate; ref is the one source of truth for scroll
  // callbacks (callback closures don't see fresh state, but here we must immediately
  // know "already frozen" within the same scroll-event batch).
  const [frozenOrder, setFrozenOrder] = useState<string[] | null>(null);
  const frozenRef = useRef<string[] | null>(null);
  /** Most recent rendered key sequence—used to sample during freeze, i.e., user's actual
   *  view order at freeze time. */
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

  // List displays partitioned by folder (Chat pinned to top), so the task page
  // has no directory dropdown: to see a directory, collapse others. The "Terminal"
  // button thus carries no initial directory; that page's own picker takes over.
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

  // 计数单位是「一件在做的事」，不是会话：一条接力链无论跑了多少棒都只算 1
  // （桌面端 `chainUnitKey` 同款）。链的归属范围要跟列表的分区口径一致 ——
  // 设备 + 仓库根，否则会出现「算作 1 个单位、列表里却画在两个分区各一行」。
  // 分组开关关掉时每行各算各的，正好与展开后的列表对上。
  const counts = useMemo(() => {
    const keyOf = (s: WithDevice<SessionInfo>) =>
      groupHandoff && s.handoff && s.handoff.chainLen > 1
        ? `${s.deviceId}::${repoRootPath(s.workspacePath)}::${s.handoff.chainId}`
        : null;
    const byBucket: Record<SessionMark, Array<WithDevice<SessionInfo>>> = {
      pending: [],
      done: [],
    };
    for (const s of preMark) byBucket[markBucket(s)].push(s);
    return {
      all: countChainUnits(preMark, keyOf),
      pending: countChainUnits(byBucket.pending, keyOf),
      done: countChainUnits(byBucket.done, keyOf),
    };
  }, [preMark, groupHandoff]);

  const visible = useMemo(
    () => preMark.filter((s) => markFilter === "all" || markBucket(s) === markFilter),
    [preMark, markFilter],
  );

  const setMark = useCallback(
    (s: WithDevice<SessionInfo>, mark: SessionMark | null) => {
      // Mark on the session's own machine, not the scoped one — marking on the
      // wrong host gets no effect.
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

  // Folder partitions form the top level of the list; relay-chain grouping nests inside
  // partitions (isomorphic to desktop launchpad), so buildRenderItems runs per-partition,
  // never chains sessions from two directories.
  // Both groupings produce the same shape so the list below renders one way;
  // only the heading's glyph, label and tooltip differ. "none" yields no
  // sections at all — the list renders `flatItems` instead.
  const sections = useMemo(() => {
    if (groupMode === "none") return [];
    if (groupMode === "status") {
      return groupStatusSections(visible).map((sec) => ({
        key: sec.key,
        name: bucketLabel(sec.bucket),
        // No path to show, and the label already says everything the heading
        // knows — so no tooltip rather than a misleading one.
        tooltip: "",
        tone: BUCKET_HEADER_TONE[sec.bucket],
        items: buildRenderItems(sec.sessions, groupHandoff),
      }));
    }
    return groupTaskSections(visible, { chatPathOf, multiDevice, deviceLabelOf }).map(
      (sec) => ({
        key: sec.key,
        name: sec.name,
        tooltip: sec.path,
        tone: null as string | null,
        items: buildRenderItems(sec.sessions, groupHandoff),
      }),
    );
  }, [visible, chatPathOf, multiDevice, deviceLabelOf, groupHandoff, groupMode]);

  // "none" mode's single stream: the same cards, relay chains still folded, in
  // the activity order `visible` already carries.
  const flatItems = useMemo(
    () => buildRenderItems(visible, groupHandoff),
    [visible, groupHandoff],
  );

  // Collapsed partition keys. Default all expanded—phone users should see sessions
  // on entry. Like search/filter, persists to localStorage: tab switches unmount this view,
  // and without persistence collapse state resets to all-expanded on every return.
  // Set can't JSON-serialize, so we store a string array on disk.
  const [collapsedKeys, setCollapsedKeys] = useDraft<string[]>("tasks:collapsedSections", []);
  const collapsedSections = useMemo(() => new Set(collapsedKeys), [collapsedKeys]);
  const toggleSection = useCallback(
    (key: string) => {
      setCollapsedKeys((prev) =>
        prev.includes(key) ? prev.filter((k) => k !== key) : [...prev, key],
      );
    },
    [setCollapsedKeys],
  );

  const handleStop = useCallback(
    async (s: WithDevice<SessionInfo>) => {
      // pid and workspacePath only make sense on **their own machine**: sent to a different
      // device, stop may fail at best, or at worst targets an unrelated process by pid.
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

  // ——— Scroll position stabilization (see savedTasksScrollY comment at file top) ———
  const restore = useCallback(() => {
    // Only restore when "already accidentally back at top, but saved position is below,
    // and page is long enough to hold it"—this way users who intentionally scroll to top
    // won't be yanked down, and normal refresh with unfallen content (scrollY != 0) isn't
    // disturbed.
    if (window.scrollY < 4 && savedTasksScrollY > 4 && maxScroll() >= savedTasksScrollY - 4) {
      window.scrollTo(0, savedTasksScrollY);
    }
  }, []);

  // Wait 150ms after scroll stops to record "user's true position". If an external event
  // instantly resets scrollY to 0, the restore logic below corrects it before that
  // 150ms fires, so the momentary zero never gets recorded.
  //
  // The same listener also manages sort-order freezing: the first scroll event snapshots
  // the current key sequence; each event thereafter pushes back the thaw timer; after
  // ORDER_FREEZE_MS of user inaction, order resorting resumes.
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

  // On mount (tab return to task page), unconditionally restore to last scroll position—at
  // this moment window.scrollY belongs to the tab we just left, not the task page's intent.
  // The list renders full height synchronously when all.length > 0, so useLayoutEffect can
  // position immediately before paint, flicker-free.
  useLayoutEffect(() => {
    if (savedTasksScrollY > 4 && maxScroll() >= savedTasksScrollY - 4) {
      window.scrollTo(0, savedTasksScrollY);
    }
  }, []);

  // After each data refresh, if scrollY was accidentally reset to top by external events,
  // correct it (guard ensures it only moves on true return-to-top).
  useLayoutEffect(() => {
    restore();
  }, [sessions, restore]);

  // Do the same when PWA returns from background—iOS often resets document scroll on
  // foreground restoration.
  useEffect(() => {
    const onVisible = () => {
      if (document.visibilityState === "visible") restore();
    };
    document.addEventListener("visibilitychange", onVisible);
    return () => document.removeEventListener("visibilitychange", onVisible);
  }, [restore]);

  // Distinguish reasons the list is empty so a blank screen never leaves users
  // guessing "is this broken or loading?". Order matters: connectivity first
  // (can we even get data?), then whether first snapshot arrived, then genuine "no
  // tasks". Only the last is truly empty; others are transient/actionable states with
  // own messaging and spinner.
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
          {/* Directory name no longer repeats per-row—it's in the partition header for
              this card's section. */}
          {/* In merged lists, device must be recognizable at a glance—same-named projects
              on two machines are common, and opening fetches that machine's transcript. */}
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
          {/* 卡死了要说卡在哪个工具上:光一个红点等于让人开着 transcript 才知道
              该不该去中断。挂了多久同样是判据——WebFetch 卡 20 分钟是坏了,
              Bash 跑 20 分钟可能只是在编译。 */}
          {s.status === "stuck" && s.stuckTool && (
            <span className={styles.remoteLost}>
              <TimerOff size={11} />
              {s.stuckTool.sinceMs
                ? `${s.stuckTool.name} ${formatWatchElapsed(s.stuckTool.sinceMs)}`
                : t("{0} 卡住", s.stuckTool.name)}
            </span>
          )}
          {/* Remote SSH tunnel died and Fleet stopped the session—a red dot alone would
              look like a generic error; must explicitly say which machine is gone. */}
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
          {/* Output ended up on the wrong machine—the session itself is fine, silent until
              mentioned. */}
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
          {/* Account credits exhausted—no status field (no reset time either; waiting for
              recharge), so on phone this looks identical to normal completion; only this
              badge says otherwise. Solid badge (not outlined like the two above) signals:
              this state won't resolve on its own. */}
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

  /** One list entry: a standalone card, or a folded relay chain that expands to
   *  its earlier hops. Shared by the grouped sections and the ungrouped stream
   *  so a card renders identically either way. */
  const renderItem = (item: (typeof flatItems)[number]) => {
    if (item.kind === "single") return renderCard(item.session);
    const { chainId, tip, members, key } = item;
    const full = chainMembersAll.get(chainId) ?? members;
    const expanded = expandedChains.has(chainId);
    const limit = chainLoadMore[chainId] ?? GROUP_VISIBLE;
    // The header card *is* the tip (latest hop); the expanded list shows only
    // the chain's *other* hops, never the tip again.
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
              <button className={styles.groupMore} onClick={() => loadMoreChain(chainId)}>
                {t("显示更早的 {0} 棒", hidden)}
              </button>
            )}
          </div>
        )}
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
        <div className={styles.segmentRow}>
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
        {/* What the sections below stand for — the same three the desktop task
            rail offers, sized down to icons because a phone has no room for
            three labels beside the mark filter. */}
        <div className={styles.segment} aria-label={t("分组方式")} role="group">
          {(["workspace", "status", "none"] as TaskGroupMode[]).map((mode) => {
            const label =
              mode === "workspace"
                ? t("按仓库分组")
                : mode === "status"
                  ? t("按状态分组")
                  : t("不分组");
            return (
              <button
                key={mode}
                className={styles.segmentButton}
                data-active={groupMode === mode}
                onClick={() => setGroupMode(mode)}
                aria-label={label}
                title={label}
              >
                {mode === "workspace" && <Folder size={15} />}
                {mode === "status" && <Activity size={15} />}
                {mode === "none" && <List size={15} />}
              </button>
            );
          })}
        </div>
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
                title={section.tooltip}
                onClick={() => toggleSection(section.key)}
              >
                {section.tone ? (
                  <span className={styles.statusDot} data-tone={section.tone} />
                ) : (
                  <Folder size={13} className={styles.workspaceFolder} />
                )}
                <span className={styles.workspaceName}>{section.name}</span>
                {/* Item count when collapsed: a collapsed handoff chain counts as one,
                    same meaning as the desktop secondary sidebar. */}
                <span className={styles.workspaceCount}>{section.items.length}</span>
                <ChevronRight
                  size={14}
                  className={styles.workspaceChevron}
                  data-open={!collapsed}
                />
              </button>
              {!collapsed && section.items.map(renderItem)}
            </section>
          );
        })}
        {/* Ungrouped: the same cards in one stream, no headings. */}
        {groupMode === "none" && flatItems.map(renderItem)}
      </div>

    </div>
  );
}
