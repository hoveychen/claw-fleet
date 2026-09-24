import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, GitBranch, Link2, ListTree, Moon, RefreshCw, TriangleAlert, X } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { PageShell } from "./PageShell";
import { HandoffChainModal } from "./HandoffChainModal";
import { distinctWorkspaces } from "./NewSessionForm";
import {
  cellStates,
  matrixMetrics,
  matrixRows,
  nodeKey,
  nodePresence,
  pendingOf,
  subtreeHasPending,
  treeRollup,
} from "./planMatrix";
import type { MatrixMetrics, MatrixRow, Presence, TreeRollup } from "./planMatrix";
import { useDetailStore, useSessionsStore, useUIStore } from "../store";
import { preferredSessionTitle } from "../types";
import type {
  AttendanceState,
  HandoffChain,
  PlanAttendance,
  PlanForest,
  PlanNode,
  ReviveOutlook,
  SessionInfo,
} from "../types";
import { TaskLine, taskTip } from "./TaskLine";
import styles from "./PlansView.module.css";

// ── Helpers ──────────────────────────────────────────────────────────────────

/** Ordered session ids on a chain, derived from its links — mirrors the Rust
 *  `HandoffChain::session_ids`. Used for the "relay leg N" count. */
function chainLegCount(chain: HandoffChain): number {
  const ids: string[] = [];
  for (const l of chain.links) {
    if (ids[ids.length - 1] !== l.fromSessionId) ids.push(l.fromSessionId);
    ids.push(l.toSessionId);
  }
  return ids.length;
}

/** Total plans in a subtree, for the "Completed N" fold's count. */
function subtreeSize(node: PlanNode): number {
  return 1 + node.children.reduce((n, c) => n + subtreeSize(c), 0);
}

/** Attendance is live state, so the board re-reads it on this cadence — the
 *  reviver's own tick. */
const LIVE_REFRESH_MS = 30_000;

type TFn = ReturnType<typeof useTranslation>["t"];

/** What a session is called on the board: its title, else a short id. */
function sessionLabel(sessions: SessionInfo[], id: string): string {
  const s = sessions.find((x) => x.id === id);
  return (s && preferredSessionTitle(s)) ?? id.slice(0, 8);
}

function stateLabel(t: TFn, state: AttendanceState): string {
  const labels: Record<AttendanceState, [string, string]> = {
    running: ["plans.att_running", "运行中"],
    watching: ["plans.att_watching", "挂着 watch 等条件"],
    scheduled: ["plans.att_scheduled", "已定时"],
    waitingCard: ["plans.att_waiting_card", "等您回复决策卡"],
    handingOff: ["plans.att_handing_off", "正在接力"],
    idle: ["plans.att_idle", "已停止"],
    bossClosed: ["plans.att_boss_closed", "已被您结束"],
    stale: ["plans.att_stale", "超过 7 天没人认领"],
  };
  const [key, fallback] = labels[state];
  return t(key, fallback);
}

function outlookLabel(t: TFn, outlook: ReviveOutlook | null | undefined, now: number): string | null {
  if (!outlook) return null;
  switch (outlook.kind) {
    case "revive": {
      const mins = Math.ceil((outlook.at - now) / 60_000);
      return mins <= 0
        ? t("plans.revive_soon", "Fleet 即将起新会话接手")
        : t("plans.revive_in", { count: mins, defaultValue: "约 {{count}} 分钟后 Fleet 起新会话接手" });
    }
    case "askBoss":
      return t("plans.revive_ask", "Fleet 会先发卡问您要不要接着做");
    case "asked":
      return t("plans.revive_asked", "Fleet 已发卡，等您决定");
    case "disabled":
      return t("plans.revive_disabled", "自动唤醒已关闭，不会有人接手");
  }
}

/** One line on who is (or was) on a plan, for tooltips and the drawer. */
function attendanceLine(
  t: TFn,
  sessions: SessionInfo[],
  a: PlanAttendance,
  outlook: ReviveOutlook | null | undefined,
): string {
  const head = `${sessionLabel(sessions, a.sessionId)} · ${stateLabel(t, a.state)}`;
  const tail = outlookLabel(t, outlook, Date.now());
  return tail ? `${head}\n${tail}` : head;
}

/** Every node in the forest, flat — resolves a selection back to its plan. */
function flatten(roots: PlanNode[], out: PlanNode[] = []): PlanNode[] {
  for (const r of roots) {
    out.push(r);
    flatten(r.children, out);
  }
  return out;
}

// ── Root view ────────────────────────────────────────────────────────────────

/**
 * Plan tree — a workspace's plans as a progress matrix: one row per plan, one cell
 * per P-task, columns aligned so you can read across rows. Server-side join
 * lives in `claw_fleet_core::plan_forest`; this view only orders and paints it.
 *
 * Two things drive the shape:
 *
 *   1. Not one line of P-task prose is on the board. Items are routinely
 *      multi-paragraph implementation notes, and rendering them inline (what
 *      this view used to do) turned the page into a wall of text. Prose lives
 *      in the right-hand drawer, behind a click, one plan at a time — and even
 *      there each item is clamped to a line until you open it.
 *   2. A matrix, not a node graph, because the forest is mostly singletons:
 *      agent-workspace carries 143 plans with 9 `parent=` links total. The
 *      `parent` links that do exist show up as indentation, and a plan with
 *      children can be collapsed.
 */
export function PlansView() {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const { selectedWorkspace, query, expandOverrides, showCompletedRoots, doneItemsShown } =
    useUIStore((s) => s.mainViewState.plans);
  const updatePlansView = useUIStore((s) => s.updatePlansView);

  const [forest, setForest] = useState<PlanForest | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [openChain, setOpenChain] = useState<HandoffChain | null>(null);
  // Which plan the drawer shows, and which of its items to open on arrival
  // (set when the click landed on a specific cell). Deliberately not persisted:
  // a drawer restored on boot would reopen prose nobody asked to see.
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [focusItem, setFocusItem] = useState<number | null>(null);

  // Repos to offer. Same derivation as the new-session launcher: worktree
  // checkouts fold onto their repo root (a plan lives in the root's TASKS.md,
  // and the backend already merges sibling worktrees), temp cwds are dropped.
  const workspaces = useMemo(() => distinctWorkspaces(sessions, 60), [sessions]);
  const shownWorkspaces = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return workspaces;
    return workspaces.filter(
      (w) => w.name.toLowerCase().includes(q) || w.path.toLowerCase().includes(q),
    );
  }, [workspaces, query]);

  // Default to the most recently active repo — the one whose plans you are
  // most likely mid-way through. Only when nothing was restored from disk.
  useEffect(() => {
    if (selectedWorkspace || workspaces.length === 0) return;
    const newest = workspaces.reduce((a, b) => (b.lastMs > a.lastMs ? b : a));
    updatePlansView({ selectedWorkspace: newest.path });
  }, [selectedWorkspace, workspaces, updatePlansView]);

  // Jump from a relay leg into that session's detail. Mirrors ScheduleView's
  // openFiredSession: the detail store needs the SessionInfo from the global
  // scan, and a leg whose transcript is gone falls back to the session list.
  const openSession = useCallback((sessionId: string) => {
    const s = useSessionsStore.getState().sessions.find((x) => x.id === sessionId);
    if (s) {
      setOpenChain(null);
      useDetailStore.getState().open(s);
    } else {
      useUIStore.getState().setViewMode(useUIStore.getState().lastSessionViewMode);
    }
  }, []);

  const load = useCallback(
    async (silent = false) => {
      if (!selectedWorkspace) return;
      if (!silent) setLoading(true);
      try {
        setForest(await invoke<PlanForest>("get_plan_forest", { workspacePath: selectedWorkspace }));
        setError(null);
      } catch (e) {
        if (!silent) {
          setForest(null);
          setError(String(e));
        }
      } finally {
        if (!silent) setLoading(false);
      }
    },
    [selectedWorkspace],
  );

  useEffect(() => {
    void load();
    const timer = setInterval(() => void load(true), LIVE_REFRESH_MS);
    return () => clearInterval(timer);
  }, [load]);

  const [unattendedOnly, setUnattendedOnly] = useState(false);

  // Dropping the selection on a repo switch keeps the drawer from showing a
  // plan that is no longer on the board.
  useEffect(() => setSelectedKey(null), [selectedWorkspace]);

  const select = (key: string, item: number | null) => {
    setSelectedKey(key);
    setFocusItem(item);
  };

  const toggleSubtree = (key: string, open: boolean) =>
    updatePlansView({ expandOverrides: { ...expandOverrides, [key]: open } });

  const toggleDoneItems = (key: string) =>
    updatePlansView({
      doneItemsShown: doneItemsShown.includes(key)
        ? doneItemsShown.filter((x) => x !== key)
        : [...doneItemsShown, key],
    });

  const { liveRoots, doneRoots, donePlanCount } = useMemo(() => {
    const roots = forest?.roots ?? [];
    const live = roots.filter(subtreeHasPending);
    const done = roots.filter((r) => !subtreeHasPending(r));
    return {
      liveRoots: live,
      doneRoots: done,
      donePlanCount: done.reduce((n, r) => n + subtreeSize(r), 0),
    };
  }, [forest]);

  const rollups = useMemo(() => {
    const m = new Map<string, TreeRollup>();
    for (const r of forest?.roots ?? []) m.set(nodeKey(r), treeRollup(r));
    return m;
  }, [forest]);
  const isUnattended = useCallback(
    (r: PlanNode) => {
      const p = rollups.get(nodeKey(r))?.presence;
      return p === "orphan" || p === "stale";
    },
    [rollups],
  );
  const unattendedCount = useMemo(() => liveRoots.filter(isUnattended).length, [liveRoots, isUnattended]);
  const activeSessionCount = useMemo(() => {
    const ids = new Set<string>();
    for (const r of rollups.values()) for (const n of r.active) ids.add(n.attendance!.sessionId);
    return ids.size;
  }, [rollups]);

  const shownRoots = useMemo(() => {
    if (unattendedOnly) return liveRoots.filter(isUnattended);
    return showCompletedRoots ? [...liveRoots, ...doneRoots] : liveRoots;
  }, [liveRoots, doneRoots, showCompletedRoots, unattendedOnly, isUnattended]);

  // A subtree stays open unless the user said otherwise; the default folds away
  // branches with nothing left to do.
  const collapsed = useMemo(() => {
    const out = new Set<string>();
    const walk = (n: PlanNode) => {
      const key = nodeKey(n);
      const open = expandOverrides[key] ?? subtreeHasPending(n);
      if (!open) out.add(key);
      else n.children.forEach(walk);
    };
    shownRoots.forEach(walk);
    return out;
  }, [shownRoots, expandOverrides]);

  const rows = useMemo(() => matrixRows(shownRoots, collapsed), [shownRoots, collapsed]);

  // Narrow windows: the title column and then the cells give ground so the
  // widest row still fits. Re-measure on every resize — opening the drawer
  // takes 340px off the board and counts as one.
  const matrixRef = useRef<HTMLDivElement | null>(null);
  const [boardW, setBoardW] = useState(0);
  useEffect(() => {
    const el = matrixRef.current;
    if (!el) return;
    const measure = () => setBoardW(el.clientWidth);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [forest]);
  const metrics = useMemo(
    () => matrixMetrics(boardW, rows.reduce((n, r) => Math.max(n, r.node.total), 0)),
    [boardW, rows],
  );

  const selected = useMemo(() => {
    if (!selectedKey) return null;
    return flatten(forest?.roots ?? []).find((n) => nodeKey(n) === selectedKey) ?? null;
  }, [selectedKey, forest]);

  const { pendingPlans, pendingTasks, totalTasks } = useMemo(() => {
    const all = flatten(shownRoots);
    return {
      pendingPlans: all.filter((n) => pendingOf(n) > 0).length,
      pendingTasks: all.reduce((n, p) => n + pendingOf(p), 0),
      totalTasks: all.reduce((n, p) => n + p.total, 0),
    };
  }, [shownRoots]);

  return (
    <PageShell
      view="plans"
      title={t("plans.title", "计划树")}
      count={forest ? liveRoots.length : null}
      search={{
        value: query,
        onChange: (v) => updatePlansView({ query: v }),
        placeholder: t("plans.search_placeholder", "筛选仓库…"),
      }}
      bannerCenter={
        <button className={styles.refresh} onClick={() => void load()} title={t("plans.refresh", "刷新")}>
          <RefreshCw size={14} strokeWidth={2} className={loading ? styles.spin : undefined} />
        </button>
      }
      secondary={
        <div className={styles.rail}>
          <div className={styles.rail_label}>{t("plans.workspaces", "仓库")}</div>
          {shownWorkspaces.map((ws) => (
            <button
              key={ws.path}
              className={`${styles.rail_item} ${ws.path === selectedWorkspace ? styles.rail_active : ""}`}
              title={ws.path}
              onClick={() => updatePlansView({ selectedWorkspace: ws.path })}
            >
              {ws.name}
            </button>
          ))}
        </div>
      }
    >
      <div className={styles.main}>
        <div className={styles.board}>
          {error && <div className={styles.error}>{error}</div>}

          {!error && forest && liveRoots.length === 0 && doneRoots.length === 0 && (
            <EmptyState
              icon={<ListTree size={28} strokeWidth={1.5} />}
              title={t("plans.empty_title", "这个仓库还没有计划")}
              subtitle={t("plans.empty_sub", "用 `fleet plan create <id> --root --title \"…\"` 开一棵计划树")}
            />
          )}

          {!error && forest && (liveRoots.length > 0 || doneRoots.length > 0) && (
            <>
              <div className={styles.toolbar}>
                <span className={styles.stat}>
                  <b>{pendingPlans}</b>
                  {t("plans.stat_pending", "个计划有待办")}
                </span>
                <span className={styles.stat_dim}>
                  {t("plans.stat_tasks", {
                    pending: pendingTasks,
                    total: totalTasks,
                    defaultValue: "{{pending}} / {{total}} 个 P 待办",
                  })}
                </span>
                {activeSessionCount > 0 && (
                  <span className={styles.stat_dim}>
                    {t("plans.stat_sessions", {
                      count: activeSessionCount,
                      defaultValue: "{{count}} 个会话在做",
                    })}
                  </span>
                )}
                {(unattendedCount > 0 || unattendedOnly) && (
                  <button
                    className={`${styles.chip} ${styles.chip_warn} ${unattendedOnly ? styles.chip_on : ""}`}
                    onClick={() => setUnattendedOnly((v) => !v)}
                    title={t("plans.unattended_filter_tip", "只看没有会话在负责的计划树")}
                  >
                    {t("plans.unattended_filter", {
                      count: unattendedCount,
                      defaultValue: "无人负责 {{count}} 棵",
                    })}
                  </button>
                )}
                {doneRoots.length > 0 && !unattendedOnly && (
                  <button
                    className={`${styles.chip} ${showCompletedRoots ? styles.chip_on : ""}`}
                    onClick={() => updatePlansView({ showCompletedRoots: !showCompletedRoots })}
                  >
                    {t("plans.completed_fold", {
                      count: donePlanCount,
                      defaultValue: "已完成 {{count}} 个",
                    })}
                  </button>
                )}
                <span className={styles.spacer} />
                <span className={styles.legend}>
                  <i className={styles.presence_active} />
                  {t("plans.legend_active", "有人在做")}
                  <i className={styles.presence_orphan} />
                  {t("plans.legend_orphan", "无人负责")}
                  <span className={styles.legend_gap} />
                  <i className={styles.dot_done} />
                  {t("plans.legend_cell_done", "已完成")}
                  <i className={styles.dot_next} />
                  {t("plans.legend_cell_next", "下一个")}
                  <i className={styles.dot_todo} />
                  {t("plans.legend_cell_todo", "待办")}
                </span>
              </div>

              <div className={styles.matrix} ref={matrixRef}>
                {rows.map((row) => (
                  <PlanMatrixRow
                    key={row.key}
                    row={row}
                    rollup={row.depth === 0 ? rollups.get(row.key) : undefined}
                    sessions={sessions}
                    onOpenSession={openSession}
                    metrics={metrics}
                    selected={row.key === selectedKey}
                    onSelect={select}
                    onToggleSubtree={toggleSubtree}
                  />
                ))}
              </div>
            </>
          )}

          {(forest?.unattachedChains.length ?? 0) > 0 && (
            <div className={styles.section}>
              <div className={styles.section_label}>{t("plans.unattached", "未挂靠的接力链")}</div>
              <div className={styles.chain_row}>
                {forest?.unattachedChains.map((c) => (
                  <ChainChip key={c.chainId} chain={c} onOpen={setOpenChain} />
                ))}
              </div>
            </div>
          )}

          {(forest?.anonymous ?? 0) > 0 && (
            <div className={styles.footnote}>
              {t("plans.anonymous", {
                count: forest?.anonymous ?? 0,
                defaultValue: "另有 {{count}} 个旧版匿名计划块（无 id，无法入树）",
              })}
            </div>
          )}
        </div>

        {selected && (
          <PlanDrawer
            node={selected}
            focusItem={focusItem}
            doneShown={doneItemsShown.includes(nodeKey(selected))}
            onToggleDone={() => toggleDoneItems(nodeKey(selected))}
            onOpenChain={setOpenChain}
            sessions={sessions}
            onOpenSession={openSession}
            onClose={() => setSelectedKey(null)}
          />
        )}
      </div>

      {openChain && (
        <HandoffChainModal
          chain={openChain}
          loading={false}
          currentSessionId=""
          hop={chainLegCount(openChain)}
          len={chainLegCount(openChain)}
          onClose={() => setOpenChain(null)}
          onOpenSession={openSession}
        />
      )}
    </PageShell>
  );
}

// ── One row of the matrix ────────────────────────────────────────────────────

interface RowProps {
  row: MatrixRow;
  /** Whole-tree summary; only on root rows. */
  rollup?: TreeRollup;
  sessions: SessionInfo[];
  onOpenSession: (sessionId: string) => void;
  metrics: MatrixMetrics;
  selected: boolean;
  onSelect: (key: string, item: number | null) => void;
  onToggleSubtree: (key: string, open: boolean) => void;
}

function PlanMatrixRow({
  row,
  rollup,
  sessions,
  onOpenSession,
  metrics,
  selected,
  onSelect,
  onToggleSubtree,
}: RowProps) {
  const { t } = useTranslation();
  const { node } = row;
  const pending = pendingOf(node);
  const cells = cellStates(node);
  const presence = nodePresence(node);
  const att = node.attendance;
  // A tree nobody is on says so on its root, even when the dot sits further
  // down on a collapsed child.
  const treeBadge =
    rollup?.presence === "orphan"
      ? {
          text: t("plans.badge_orphan", "无人负责"),
          tip: rollup.next?.attendance
            ? attendanceLine(t, sessions, rollup.next.attendance, rollup.next.revive)
            : "",
          cls: styles.badge_orphan,
        }
      : rollup?.presence === "stale"
        ? {
            text: t("plans.badge_stale", "久未认领"),
            tip: t("plans.badge_stale_tip", "有待办，但最近 7 天没有会话认领过，Fleet 不会自动唤醒"),
            cls: styles.badge_stale,
          }
        : null;

  return (
    <div className={`${styles.row} ${selected ? styles.row_selected : ""}`}>
      <button
        className={styles.row_head}
        style={{ width: metrics.titleW, paddingLeft: 6 + row.depth * 16 }}
        title={`${node.title || node.id}\n${node.id}${node.source ? `\n${node.source}` : ""}`}
        onClick={() => onSelect(row.key, null)}
      >
        {row.hasChildren ? (
          <span
            className={styles.caret_hit}
            role="button"
            tabIndex={-1}
            title={
              row.collapsed
                ? t("plans.expand_subtree", {
                    count: row.hiddenDescendants,
                    defaultValue: "展开 {{count}} 个子计划",
                  })
                : t("plans.collapse_subtree", "折叠子树")
            }
            onClick={(e) => {
              e.stopPropagation();
              onToggleSubtree(row.key, row.collapsed);
            }}
          >
            <ChevronRight
              size={12}
              strokeWidth={2}
              className={row.collapsed ? styles.caret : styles.caret_open}
            />
          </span>
        ) : (
          <span className={styles.caret_hit} aria-hidden />
        )}
        {node.orphanedParent ? (
          <TriangleAlert size={11} strokeWidth={2} className={styles.orphan} />
        ) : (
          <PresenceDot presence={presence} />
        )}
        <span className={pending === 0 ? styles.title_done : styles.title}>
          {node.title || node.id}
        </span>
        {node.snooze && (
          <span className={styles.snooze} title={node.snooze.reason}>
            <Moon size={10} strokeWidth={2} />
          </span>
        )}
        {treeBadge && (
          <span className={`${styles.badge} ${treeBadge.cls}`} title={treeBadge.tip || undefined}>
            {treeBadge.text}
          </span>
        )}
        {att && (presence === "active" || presence === "orphan") && (
          <span
            className={presence === "active" ? styles.owner : styles.owner_gone}
            role="button"
            tabIndex={-1}
            title={attendanceLine(t, sessions, att, node.revive)}
            onClick={(e) => {
              e.stopPropagation();
              onOpenSession(att.sessionId);
            }}
          >
            {sessionLabel(sessions, att.sessionId)}
          </span>
        )}
      </button>

      <span className={styles.chain_slot}>
        {node.chains.length > 0 && (
          <span className={styles.chain_count}>
            <Link2 size={10} strokeWidth={2} />
            {node.chains.length}
          </span>
        )}
      </span>
      <span className={pending > 0 ? styles.count_live : styles.count}>
        {node.done}/{node.total}
      </span>

      <span className={styles.cells} style={{ gap: metrics.gap }}>
        {cells.map((state, i) => (
          <button
            key={i}
            className={`${styles.cell} ${styles[`cell_${state}`]}`}
            style={{ width: metrics.cellW }}
            title={node.items[i] ? taskTip(node.items[i].text) : `P${i + 1}`}
            onClick={() => onSelect(row.key, i)}
          />
        ))}
      </span>
    </div>
  );
}

function PresenceDot({ presence }: { presence: Presence }) {
  const cls = {
    active: styles.presence_active,
    orphan: styles.presence_orphan,
    stale: styles.presence_stale,
    snoozed: null,
    none: null,
  }[presence];
  // Keep the slot when there is no dot, so titles stay aligned.
  return <i className={cls ?? styles.presence_none} />;
}

function formatSnoozeUntil(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

// ── Detail drawer ────────────────────────────────────────────────────────────

interface DrawerProps {
  node: PlanNode;
  /** Item index to open on arrival — set when a cell, not the row, was clicked. */
  focusItem: number | null;
  doneShown: boolean;
  onToggleDone: () => void;
  onOpenChain: (chain: HandoffChain) => void;
  sessions: SessionInfo[];
  onOpenSession: (sessionId: string) => void;
  onClose: () => void;
}

/** The only place P-task prose is rendered: one plan, on demand. */
function PlanDrawer({
  node,
  focusItem,
  doneShown,
  onToggleDone,
  onOpenChain,
  sessions,
  onOpenSession,
  onClose,
}: DrawerProps) {
  const { t } = useTranslation();
  // Plan titles are supposed to be one line, but plenty in the wild are a whole
  // paragraph — clamp like the items do rather than let one push the tasks off.
  const [titleOpen, setTitleOpen] = useState(false);
  const indexed = node.items.map((item, i) => ({ item, i }));
  const pendingItems = indexed.filter((x) => !x.item.done);
  const doneItems = indexed.filter((x) => x.item.done);
  const focusIsDone = focusItem != null && node.items[focusItem]?.done === true;

  return (
    <aside className={styles.drawer}>
      <div className={styles.drawer_head}>
        <div
          className={titleOpen ? styles.drawer_title_open : styles.drawer_title}
          onClick={() => setTitleOpen((v) => !v)}
          title={node.title ?? undefined}
        >
          {node.title || node.id}
        </div>
        <button className={styles.close} onClick={onClose} title={t("plans.close", "关闭")}>
          <X size={14} strokeWidth={2} />
        </button>
      </div>
      <div className={styles.drawer_meta}>
        <span className={styles.id}>{node.id}</span>
        {node.kind === "explore" && <span className={styles.kind}>explore</span>}
        <span className={pendingOf(node) > 0 ? styles.count_live : styles.count}>
          {node.done}/{node.total}
        </span>
      </div>
      {node.source && <div className={styles.source}>{node.source}</div>}
      {node.orphanedParent && (
        <div className={styles.orphan_note}>
          <TriangleAlert size={11} strokeWidth={2} />
          {t("plans.orphan_tip", {
            parent: node.orphanedParent,
            defaultValue: "父计划 {{parent}} 不存在,已提升为根",
          })}
        </div>
      )}

      {node.attendance && (
        <button
          className={nodePresence(node) === "active" ? styles.owner_note : styles.owner_note_gone}
          onClick={() => onOpenSession(node.attendance!.sessionId)}
        >
          <PresenceDot presence={nodePresence(node)} />
          <span>{attendanceLine(t, sessions, node.attendance, node.revive)}</span>
        </button>
      )}

      {node.snooze && (
        <div className={styles.snooze_note}>
          <Moon size={11} strokeWidth={2} />
          <span>
            {node.snooze.untilMs != null
              ? t("plans.snooze_until", {
                  until: formatSnoozeUntil(node.snooze.untilMs),
                  defaultValue: "静默至 {{until}}",
                })
              : t("plans.snooze_forever", "已停止自动唤醒")}
            {node.snooze.reason && ` · ${node.snooze.reason}`}
          </span>
        </div>
      )}

      <div className={styles.drawer_body}>
        {pendingItems.map(({ item, i }, n) => (
          <TaskLine
            key={`p${i}`}
            text={item.text}
            state={n === 0 ? "current" : "pending"}
            startOpen={i === focusItem}
          />
        ))}
        {doneItems.length > 0 && (
          <button className={styles.done_fold} onClick={onToggleDone}>
            <ChevronRight
              size={12}
              strokeWidth={2}
              className={doneShown ? styles.caret_open : styles.caret}
            />
            {t("plans.done_items", { count: doneItems.length, defaultValue: "已完成 {{count}} 条" })}
          </button>
        )}
        {/* Clicking a finished cell has to reveal it even while the fold is shut,
            otherwise the click looks broken. */}
        {(doneShown || focusIsDone) &&
          doneItems
            .filter(({ i }) => doneShown || i === focusItem)
            .map(({ item, i }) => (
              <TaskLine key={`d${i}`} text={item.text} state="done" startOpen={i === focusItem} />
            ))}
        {node.chains.map((c) => (
          <ChainChip key={c.chainId} chain={c} onOpen={onOpenChain} />
        ))}
      </div>
    </aside>
  );
}

/** A whole relay folded to one node: click to see every leg in the shared
 *  handoff modal (same rendering the session detail uses). */
function ChainChip({ chain, onOpen }: { chain: HandoffChain; onOpen: (c: HandoffChain) => void }) {
  const { t } = useTranslation();
  const legs = chainLegCount(chain);
  return (
    <button className={styles.chain} onClick={() => onOpen(chain)}>
      <GitBranch size={12} strokeWidth={2} />
      {t("plans.relay", { count: legs, defaultValue: "接力 {{count}} 棒" })}
      <span className={styles.chain_id}>{chain.chainId.slice(0, 8)}</span>
    </button>
  );
}
