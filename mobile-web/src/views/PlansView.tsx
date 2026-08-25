// 仓库级「计划」页：桌面端计划树的手机版。一行一个计划、一格一个 P-task，
// 列对齐；点行或点格子从底部升起详情，正文只在那里出现。数据走 relay 的
// `plan_forest`（claw-fleet-core/src/mobile_relay.rs），和桌面 计划树 同源 ——
// 会话详情里的「任务计划」页签用的是 `task_plans`，那是扁平的按会话列表，
// 没有 parent 关系、没有 done/total、没有接力链。

import { useCallback, useEffect, useMemo, useState } from "react";
import { ChevronLeft, ChevronRight, GitBranch, ListTree, TriangleAlert, X } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { PlanForest, PlanNode, SessionInfo } from "../types";
import {
  cellStates,
  matrixMetrics,
  matrixRows,
  nodeKey,
  pendingOf,
  splitMarker,
  subtreeHasPending,
} from "./planMatrix";
import styles from "./PlansView.module.css";

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

/** Repos to offer, newest first. Derived from the sessions the phone already
 *  holds rather than `repo_list` — that one shells out to git for every repo,
 *  and all this page needs is a path to read TASKS.md under. */
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
  // Measured on the row strip's container: a phone in portrait has ~330px to
  // spend and the metrics decide how it is split between title and cells.
  const [boardW, setBoardW] = useState(0);

  useEffect(() => {
    if (!repo && repos.length > 0) setRepo(repos[0].path);
  }, [repo, repos]);

  const load = useCallback(async () => {
    if (!client || !repo) return;
    setLoading(true);
    setError(null);
    try {
      setForest(await client.request<PlanForest>("plan_forest", { workspacePath: repo }));
    } catch (e) {
      setForest(null);
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setLoading(false);
    }
  }, [client, repo]);

  useEffect(() => {
    setSelectedKey(null);
    void load();
  }, [load]);

  const { liveRoots, doneRoots, donePlanCount } = useMemo(() => {
    const roots = forest?.roots ?? [];
    const live = roots.filter(subtreeHasPending);
    const done = roots.filter((r) => !subtreeHasPending(r));
    return { liveRoots: live, doneRoots: done, donePlanCount: done.reduce((n, r) => n + subtreeSize(r), 0) };
  }, [forest]);

  const shownRoots = useMemo(
    () => (showDone ? [...liveRoots, ...doneRoots] : liveRoots),
    [liveRoots, doneRoots, showDone],
  );

  // Same default as the desktop: a branch with nothing left to do folds away.
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
      <div className={styles.header}>
        <button className={styles.backButton} onClick={onBack} aria-label={t("返回")}>
          <ChevronLeft size={22} />
          {t("更多")}
        </button>
        <div className={styles.headerText}>
          <div className={styles.headerTitle}>{t("计划")}</div>
        </div>
        <button className={styles.refresh} onClick={() => void load()} aria-label={t("刷新")}>
          ⟳
        </button>
      </div>

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
              {doneRoots.length > 0 && (
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
                        row.node.kind === "explore" && <i className={styles.kindDot} />
                      )}
                      <span className={styles.title} data-done={pending === 0}>
                        {row.node.title || row.node.id}
                      </span>
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
            focusItem={focusItem}
            onClose={() => setSelectedKey(null)}
          />
        </>
      )}
    </div>
  );
}

/** The only place P-task prose appears: one plan, from the bottom, on demand. */
function PlanSheet({
  node,
  focusItem,
  onClose,
}: {
  node: PlanNode;
  focusItem: number | null;
  onClose: () => void;
}) {
  const [doneShown, setDoneShown] = useState(false);
  const [titleOpen, setTitleOpen] = useState(false);
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

      <div className={styles.sheetBody}>
        {pendingItems.map(({ item, i }) => (
          <TaskLine key={`p${i}`} text={item.text} startOpen={i === focusItem} />
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
              <TaskLine key={`d${i}`} text={item.text} done startOpen={i === focusItem} />
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

/** One P-task: `P3` badge then prose, clamped to a line until tapped. */
function TaskLine({ text, done, startOpen }: { text: string; done?: boolean; startOpen?: boolean }) {
  const { marker, rest } = splitMarker(text);
  const [open, setOpen] = useState(!!startOpen);
  return (
    <div className={styles.item} data-done={done} onClick={() => setOpen((v) => !v)}>
      <span className={styles.box}>{done ? "☑" : "☐"}</span>
      {marker && <span className={styles.marker}>{marker}</span>}
      <span className={styles.itemText} data-open={open}>
        {rest}
      </span>
    </div>
  );
}
