// Repo-level plan view: phone version of the desktop plan tree. One row per plan,
// one cell per P-task, columns aligned; tapping a row or cell brings up details from
// the bottom, with full text shown only there. Data comes from `plan_forest` via relay
// (claw-fleet-core/src/mobile_relay.rs), same source as the desktop plan tree — the
// "task plans" tab in session details uses `task_plans` instead, which is flat per-session
// list with no parent relationships, done/total, or handoff chains.

import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronRight, GitBranch, ListTree, Moon, RefreshCw, TriangleAlert, X } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import { useHistoryLayer } from "../useNavStack";
import type { FleetTransport } from "../transport";
import type {
  AttendanceState,
  PlanAttendance,
  PlanForest,
  PlanNode,
  ReviveOutlook,
  SessionInfo,
} from "../types";
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
import type { Presence } from "./planMatrix";
import { TaskItemLine } from "./TaskItemLine";
import styles from "./PlansView.module.css";
import { AppHeader } from "./AppHeader";
import { HeaderAction } from "./HeaderAction";

interface Props {
  sessions: SessionInfo[];
  client: FleetTransport | null;
  onBack: () => void;
}

interface Repo {
  path: string;
  name: string;
  lastMs: number;
}

/** Repos to offer, newest first. Derived from sessions the phone already has
 *  rather than `repo_list` — that one shells out to git for every repo, and all
 *  this page needs is a path to read TASKS.md from. */
function distinctRepos(sessions: SessionInfo[]): Repo[] {
  const byPath = new Map<string, Repo>();
  for (const s of sessions) {
    if (!s.workspacePath) continue;
    const prev = byPath.get(s.workspacePath);
    const lastMs = s.lastActivityMs ?? 0;
    if (!prev) byPath.set(s.workspacePath, { path: s.workspacePath, name: s.workspaceName, lastMs });
    else if (lastMs > prev.lastMs) prev.lastMs = lastMs;
  }
  return [...byPath.values()].sort((a, b) => b.lastMs - a.lastMs);
}

function flatten(roots: PlanNode[], out: PlanNode[] = []): PlanNode[] {
  for (const r of roots) {
    out.push(r);
    flatten(r.children, out);
  }
  return out;
}

function subtreeSize(node: PlanNode): number {
  return 1 + node.children.reduce((n, c) => n + subtreeSize(c), 0);
}

/** Attendance is live state; re-read it on the reviver's own tick. */
const LIVE_REFRESH_MS = 30_000;

const STATE_LABEL: Record<AttendanceState, string> = {
  running: "运行中",
  watching: "挂着守望等条件",
  scheduled: "已定时",
  waitingCard: "等您回复决策卡",
  handingOff: "正在接力",
  idle: "已停止",
  bossClosed: "已被您结束",
  stale: "超过 7 天没人认领",
};

function outlookLabel(outlook: ReviveOutlook | null | undefined): string | null {
  if (!outlook) return null;
  switch (outlook.kind) {
    case "revive": {
      const mins = Math.ceil((outlook.at - Date.now()) / 60_000);
      return mins <= 0 ? t("Fleet 即将起新会话接手") : t("约 {0} 分钟后 Fleet 起新会话接手", String(mins));
    }
    case "askBoss":
      return t("Fleet 会先发卡问您要不要接着做");
    case "asked":
      return t("Fleet 已发卡，等您决定");
    case "disabled":
      return t("自动唤醒已关闭，不会有人接手");
  }
}

function sessionLabel(sessions: SessionInfo[], id: string): string {
  const s = sessions.find((x) => x.id === id);
  return s?.titleOverride ?? s?.aiTitle ?? id.slice(0, 8);
}

function PresenceDot({ presence }: { presence: Presence }) {
  return <i className={styles.presence} data-presence={presence} />;
}

export function PlansView({ sessions, client, onBack }: Props) {
  const repos = useMemo(() => distinctRepos(sessions), [sessions]);
  const [repo, setRepo] = useState<string | null>(null);
  const [forest, setForest] = useState<PlanForest | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [showDone, setShowDone] = useState(false);
  const [collapsedKeys, setCollapsedKeys] = useState<string[]>([]);
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const [focusItem, setFocusItem] = useState<number | null>(null);
  // Measured on the row strip's container: a phone in portrait has ~330px available
  // and the metrics decide how to split it between title and cells.
  const [boardW, setBoardW] = useState(0);

  useEffect(() => {
    if (!repo && repos.length > 0) setRepo(repos[0].path);
  }, [repo, repos]);

  const load = useCallback(
    async (silent = false) => {
      if (!client || !repo) return;
      if (!silent) {
        setLoading(true);
        setError(null);
      }
      try {
        setForest(await client.request<PlanForest>("plan_forest", { workspacePath: repo }));
      } catch (e) {
        if (!silent) {
          setForest(null);
          setError(e instanceof Error ? e.message : String(e));
        }
      } finally {
        if (!silent) setLoading(false);
      }
    },
    [client, repo],
  );

  useEffect(() => {
    setSelectedKey(null);
    void load();
    const timer = setInterval(() => void load(true), LIVE_REFRESH_MS);
    return () => clearInterval(timer);
  }, [load]);

  const [unattendedOnly, setUnattendedOnly] = useState(false);

  const { liveRoots, doneRoots, donePlanCount } = useMemo(() => {
    const roots = forest?.roots ?? [];
    const live = roots.filter(subtreeHasPending);
    const done = roots.filter((r) => !subtreeHasPending(r));
    return { liveRoots: live, doneRoots: done, donePlanCount: done.reduce((n, r) => n + subtreeSize(r), 0) };
  }, [forest]);

  const rollups = useMemo(() => {
    const m = new Map<string, ReturnType<typeof treeRollup>>();
    for (const r of forest?.roots ?? []) m.set(nodeKey(r), treeRollup(r));
    return m;
  }, [forest]);
  const unattended = useMemo(
    () =>
      liveRoots.filter((r) => {
        const p = rollups.get(nodeKey(r))?.presence;
        return p === "orphan" || p === "stale";
      }),
    [liveRoots, rollups],
  );

  const shownRoots = useMemo(() => {
    if (unattendedOnly) return unattended;
    return showDone ? [...liveRoots, ...doneRoots] : liveRoots;
  }, [liveRoots, doneRoots, showDone, unattendedOnly, unattended]);

  // Same default as desktop: a branch with no pending tasks collapses by default.
  const collapsed = useMemo(() => {
    const overrides = new Set(collapsedKeys);
    const out = new Set<string>();
    const walk = (n: PlanNode) => {
      const key = nodeKey(n);
      const open = overrides.has(key) ? false : subtreeHasPending(n);
      if (!open) out.add(key);
      else n.children.forEach(walk);
    };
    shownRoots.forEach(walk);
    return out;
  }, [shownRoots, collapsedKeys]);

  const rows = useMemo(() => matrixRows(shownRoots, collapsed), [shownRoots, collapsed]);
  const metrics = useMemo(
    () => matrixMetrics(boardW, rows.reduce((n, r) => Math.max(n, r.node.total), 0)),
    [boardW, rows],
  );

  const selected = useMemo(
    () => (selectedKey ? flatten(forest?.roots ?? []).find((n) => nodeKey(n) === selectedKey) ?? null : null),
    [selectedKey, forest],
  );

  const { pendingTasks, totalTasks } = useMemo(() => {
    const all = flatten(shownRoots);
    return {
      pendingTasks: all.reduce((n, p) => n + pendingOf(p), 0),
      totalTasks: all.reduce((n, p) => n + p.total, 0),
    };
  }, [shownRoots]);

  const measure = useCallback((el: HTMLDivElement | null) => {
    if (el) setBoardW(el.clientWidth);
  }, []);

  const toggleSubtree = (key: string) =>
    setCollapsedKeys((keys) => (keys.includes(key) ? keys.filter((k) => k !== key) : [...keys, key]));

  return (
    <div className={styles.page}>
      <AppHeader
        onBack={onBack}
        title={t("计划")}
        // repoRow has its own border-bottom, which overlays with the header's hairline
        // to create two lines that divide the same chrome area in two (session details'
        // tab bar requires seamless for this reason). The condition: when there is only
        // one repo, this row doesn't render at all, so the header's hairline is the only
        // border, and removing it would blur the page top and content together.
        seamless={repos.length > 1}
        actions={
          <HeaderAction
            icon={<RefreshCw size={17} />}
            label={t("刷新")}
            onClick={() => void load()}
            busy={loading}
          />
        }
      />

      {repos.length > 1 && (
        <div className={styles.repoRow}>
          {repos.map((r) => (
            <button
              key={r.path}
              className={styles.repoChip}
              data-active={r.path === repo}
              onClick={() => setRepo(r.path)}
            >
              {r.name}
            </button>
          ))}
        </div>
      )}

      <div className={styles.body}>
        {error && <div className={styles.hint}>{t("计划加载失败：{0}", error)}</div>}
        {!error && loading && forest === null && <div className={styles.hint}>{t("加载中…")}</div>}
        {!error && forest !== null && liveRoots.length === 0 && doneRoots.length === 0 && (
          <EmptyState icon={ListTree} title={t("这个仓库还没有计划")} />
        )}

        {!error && forest !== null && (liveRoots.length > 0 || doneRoots.length > 0) && (
          <>
            <div className={styles.toolbar}>
              <span className={styles.stat}>
                {t("{0} 个计划有待办", String(flatten(shownRoots).filter((n) => pendingOf(n) > 0).length))}
              </span>
              <span className={styles.statDim}>{`${pendingTasks} / ${totalTasks} P`}</span>
              {(unattended.length > 0 || unattendedOnly) && (
                <button
                  className={styles.chip}
                  data-warn={!unattendedOnly}
                  data-on={unattendedOnly}
                  onClick={() => setUnattendedOnly((v) => !v)}
                >
                  {t("无人负责 {0} 棵", String(unattended.length))}
                </button>
              )}
              {doneRoots.length > 0 && !unattendedOnly && (
                <button
                  className={styles.chip}
                  data-on={showDone}
                  onClick={() => setShowDone((v) => !v)}
                >
                  {t("已完成 {0} 个", String(donePlanCount))}
                </button>
              )}
            </div>

            <div className={styles.matrix} ref={measure}>
              {rows.map((row) => {
                const cells = cellStates(row.node);
                const pending = pendingOf(row.node);
                const rollup = row.depth === 0 ? rollups.get(row.key) : undefined;
                return (
                  <div
                    key={row.key}
                    className={styles.row}
                    data-selected={row.key === selectedKey}
                    onClick={() => {
                      setSelectedKey(row.key);
                      setFocusItem(null);
                    }}
                  >
                    <div
                      className={styles.rowHead}
                      style={{ width: metrics.titleW, paddingLeft: 2 + row.depth * 12 }}
                    >
                      {row.hasChildren ? (
                        <span
                          className={styles.caret}
                          data-open={!row.collapsed}
                          onClick={(e) => {
                            e.stopPropagation();
                            toggleSubtree(row.key);
                          }}
                        >
                          <ChevronRight size={12} />
                        </span>
                      ) : (
                        <span className={styles.caret} aria-hidden />
                      )}
                      {row.node.orphanedParent ? (
                        <TriangleAlert size={10} className={styles.orphan} />
                      ) : (
                        <PresenceDot presence={nodePresence(row.node)} />
                      )}
                      <span className={styles.title} data-done={pending === 0}>
                        {row.node.title || row.node.id}
                      </span>
                      {row.node.snooze && <Moon size={10} className={styles.snooze} />}
                      {rollup?.presence === "orphan" && (
                        <span className={styles.badge} data-kind="orphan">{t("无人负责")}</span>
                      )}
                      {rollup?.presence === "stale" && (
                        <span className={styles.badge} data-kind="stale">{t("久未认领")}</span>
                      )}
                    </div>
                    <span className={styles.count} data-live={pending > 0}>
                      {row.node.done}/{row.node.total}
                    </span>
                    <span className={styles.cells} style={{ gap: metrics.gap }}>
                      {cells.map((state, i) => (
                        <span
                          key={i}
                          className={styles.cell}
                          data-state={state}
                          style={{ width: metrics.cellW }}
                          onClick={(e) => {
                            e.stopPropagation();
                            setSelectedKey(row.key);
                            setFocusItem(i);
                          }}
                        />
                      ))}
                    </span>
                  </div>
                );
              })}
            </div>
          </>
        )}
      </div>

      {selected && (
        <>
          <div className={styles.scrim} onClick={() => setSelectedKey(null)} />
          <PlanSheet
            node={selected}
            sessions={sessions}
            focusItem={focusItem}
            onClose={() => setSelectedKey(null)}
          />
        </>
      )}
    </div>
  );
}

/** The only place P-task prose appears: one plan, slid up from the bottom, on demand. */
function PlanSheet({
  node,
  sessions,
  focusItem,
  onClose,
}: {
  node: PlanNode;
  sessions: SessionInfo[];
  focusItem: number | null;
  onClose: () => void;
}) {
  const [doneShown, setDoneShown] = useState(false);
  const [titleOpen, setTitleOpen] = useState(false);
  // Like the artifact preview: sheet is a second layer overlaid on the plan page,
  // and it does not register its own history level, so back dismisses the entire
  // plan page and returns to the parent view in one step.
  useHistoryLayer(onClose);
  const indexed = node.items.map((item, i) => ({ item, i }));
  const pendingItems = indexed.filter((x) => !x.item.done);
  const doneItems = indexed.filter((x) => x.item.done);
  const focusIsDone = focusItem != null && node.items[focusItem]?.done === true;

  return (
    <div className={styles.sheet}>
      <div className={styles.sheetHead}>
        <div
          className={styles.sheetTitle}
          data-open={titleOpen}
          onClick={() => setTitleOpen((v) => !v)}
        >
          {node.title || node.id}
        </div>
        <button className={styles.close} onClick={onClose} aria-label={t("关闭")}>
          <X size={16} />
        </button>
      </div>
      <div className={styles.sheetMeta}>
        <span className={styles.id}>{node.id}</span>
        {node.kind === "explore" && <span className={styles.kind}>explore</span>}
        <span className={styles.count} data-live={pendingOf(node) > 0}>
          {node.done}/{node.total}
        </span>
      </div>
      {node.source && <div className={styles.source}>{node.source}</div>}
      {node.attendance && (
        <OwnerNote node={node} attendance={node.attendance} sessions={sessions} />
      )}
      {node.snooze && (
        <div className={styles.snoozeNote}>
          <Moon size={11} />
          <span>
            {node.snooze.untilMs != null
              ? t("静默至 {0}", formatSnoozeUntil(node.snooze.untilMs))
              : t("已停止自动唤醒")}
            {node.snooze.reason && ` · ${node.snooze.reason}`}
          </span>
        </div>
      )}

      <div className={styles.sheetBody}>
        {pendingItems.map(({ item, i }, n) => (
          <TaskItemLine
            key={`p${i}`}
            text={item.text}
            state={n === 0 ? "current" : "pending"}
            startOpen={i === focusItem}
          />
        ))}
        {doneItems.length > 0 && (
          <button className={styles.doneFold} onClick={() => setDoneShown((v) => !v)}>
            <ChevronRight size={12} className={doneShown ? styles.caretOpen : undefined} />
            {t("已完成 {0} 条", String(doneItems.length))}
          </button>
        )}
        {(doneShown || focusIsDone) &&
          doneItems
            .filter(({ i }) => doneShown || i === focusItem)
            .map(({ item, i }) => (
              <TaskItemLine key={`d${i}`} text={item.text} state="done" startOpen={i === focusItem} />
            ))}
        {node.chains.map((c) => (
          <div key={c.chainId} className={styles.chain}>
            <GitBranch size={12} />
            {t("接力 {0} 棒", String(c.links.length + 1))}
            <span className={styles.chainId}>{c.chainId.slice(0, 8)}</span>
          </div>
        ))}
      </div>
    </div>
  );
}

function OwnerNote({
  node,
  attendance,
  sessions,
}: {
  node: PlanNode;
  attendance: PlanAttendance;
  sessions: SessionInfo[];
}) {
  const presence = nodePresence(node);
  const outlook = outlookLabel(node.revive);
  return (
    <div className={styles.ownerNote} data-active={presence === "active"}>
      <PresenceDot presence={presence} />
      <span>
        {`${sessionLabel(sessions, attendance.sessionId)} · ${t(STATE_LABEL[attendance.state])}`}
        {outlook && <br />}
        {outlook}
      </span>
    </div>
  );
}

function formatSnoozeUntil(ms: number): string {
  const d = new Date(ms);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
