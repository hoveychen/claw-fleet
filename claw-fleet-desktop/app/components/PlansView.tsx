import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, GitBranch, Link2, ListTree, RefreshCw, TriangleAlert, X } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { PageShell } from "./PageShell";
import { HandoffChainModal } from "./HandoffChainModal";
import { distinctWorkspaces } from "./NewSessionForm";
import { cellStates, matrixRows, nodeKey, pendingOf, subtreeHasPending } from "./planMatrix";
import type { MatrixRow } from "./planMatrix";
import { useDetailStore, useSessionsStore, useUIStore } from "../store";
import type { HandoffChain, PlanForest, PlanNode } from "../types";
import styles from "./PlansView.module.css";

// ── Helpers ──────────────────────────────────────────────────────────────────

/** Ordered session ids on a chain, derived from its links — mirrors the Rust
 *  `HandoffChain::session_ids`. Used for the 「接力 n 棒」 count. */
function chainLegCount(chain: HandoffChain): number {
  const ids: string[] = [];
  for (const l of chain.links) {
    if (ids[ids.length - 1] !== l.fromSessionId) ids.push(l.fromSessionId);
    ids.push(l.toSessionId);
  }
  return ids.length;
}

/** Split a TASKS.md item into its `P<n>` marker and the rest.
 *
 *  Items are written `**P3** — do the thing`, so the raw string renders as
 *  literal asterisks — the P-number, the one part you scan a plan by, is the
 *  part that looks like noise. Pull it out as a badge and drop the emphasis
 *  markers from the remainder. Anything that doesn't match keeps its text
 *  verbatim rather than being mangled by a half-baked markdown pass. */
function splitMarker(text: string): { marker: string | null; rest: string } {
  const m = /^\*\*(P\d+[a-z]?)\*\*\s*(?:[—–-]\s*)?([\s\S]*)$/.exec(text.trim());
  if (!m) return { marker: null, rest: text };
  return { marker: m[1], rest: m[2].replace(/\*\*/g, "") };
}

/** Total plans in a subtree, for the 「已完成 N 个」 fold's count. */
function subtreeSize(node: PlanNode): number {
  return 1 + node.children.reduce((n, c) => n + subtreeSize(c), 0);
}

/** Every node in the forest, flat — resolves a selection back to its plan. */
function flatten(roots: PlanNode[], out: PlanNode[] = []): PlanNode[] {
  for (const r of roots) {
    out.push(r);
    flatten(r.children, out);
  }
  return out;
}

/** Cell tooltip: the P-task's own text, trimmed to something a title attribute
 *  can carry (items here run to several paragraphs). */
function cellTip(text: string): string {
  const { marker, rest } = splitMarker(text);
  const body = rest.replace(/\s+/g, " ");
  const head = marker ? `${marker} — ` : "";
  return head + (body.length > 160 ? `${body.slice(0, 160)}…` : body);
}

// ── Root view ────────────────────────────────────────────────────────────────

/**
 * 计划树 — a workspace's plans as a progress matrix: one row per plan, one cell
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

  const load = useCallback(async () => {
    if (!selectedWorkspace) return;
    setLoading(true);
    try {
      setForest(await invoke<PlanForest>("get_plan_forest", { workspacePath: selectedWorkspace }));
      setError(null);
    } catch (e) {
      setForest(null);
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [selectedWorkspace]);

  useEffect(() => {
    void load();
  }, [load]);

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

  const shownRoots = useMemo(
    () => (showCompletedRoots ? [...liveRoots, ...doneRoots] : liveRoots),
    [liveRoots, doneRoots, showCompletedRoots],
  );

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
                {doneRoots.length > 0 && (
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
                  <i className={styles.dot_done} />
                  {t("plans.legend_cell_done", "已完成")}
                  <i className={styles.dot_next} />
                  {t("plans.legend_cell_next", "下一个")}
                  <i className={styles.dot_todo} />
                  {t("plans.legend_cell_todo", "待办")}
                </span>
              </div>

              <div className={styles.matrix}>
                {rows.map((row) => (
                  <PlanMatrixRow
                    key={row.key}
                    row={row}
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
  selected: boolean;
  onSelect: (key: string, item: number | null) => void;
  onToggleSubtree: (key: string, open: boolean) => void;
}

function PlanMatrixRow({ row, selected, onSelect, onToggleSubtree }: RowProps) {
  const { t } = useTranslation();
  const { node } = row;
  const pending = pendingOf(node);
  const cells = cellStates(node);

  return (
    <div className={`${styles.row} ${selected ? styles.row_selected : ""}`}>
      <button
        className={styles.row_head}
        style={{ paddingLeft: 6 + row.depth * 16 }}
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
          node.kind === "explore" && <i className={styles.kind_dot} />
        )}
        <span className={pending === 0 ? styles.title_done : styles.title}>
          {node.title || node.id}
        </span>
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

      <span className={styles.cells}>
        {cells.map((state, i) => (
          <button
            key={i}
            className={`${styles.cell} ${styles[`cell_${state}`]}`}
            title={node.items[i] ? cellTip(node.items[i].text) : `P${i + 1}`}
            onClick={() => onSelect(row.key, i)}
          />
        ))}
      </span>
    </div>
  );
}

// ── Detail drawer ────────────────────────────────────────────────────────────

interface DrawerProps {
  node: PlanNode;
  /** Item index to open on arrival — set when a cell, not the row, was clicked. */
  focusItem: number | null;
  doneShown: boolean;
  onToggleDone: () => void;
  onOpenChain: (chain: HandoffChain) => void;
  onClose: () => void;
}

/** The only place P-task prose is rendered: one plan, on demand. */
function PlanDrawer({
  node,
  focusItem,
  doneShown,
  onToggleDone,
  onOpenChain,
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

      <div className={styles.drawer_body}>
        {pendingItems.map(({ item, i }) => (
          <TaskLine key={`p${i}`} text={item.text} startOpen={i === focusItem} />
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
              <TaskLine key={`d${i}`} text={item.text} done startOpen={i === focusItem} />
            ))}
        {node.chains.map((c) => (
          <ChainChip key={c.chainId} chain={c} onOpen={onOpenChain} />
        ))}
      </div>
    </aside>
  );
}

/** One P-task line: `P3` as a badge, then its prose — clamped to a single line
 *  until clicked. Items here run to several paragraphs of implementation notes;
 *  showing them all in full is the thing this redesign exists to stop. */
function TaskLine({ text, done, startOpen }: { text: string; done?: boolean; startOpen?: boolean }) {
  const { marker, rest } = splitMarker(text);
  const [open, setOpen] = useState(!!startOpen);
  return (
    <button
      className={`${styles.item} ${done ? styles.item_done : ""}`}
      onClick={() => setOpen((v) => !v)}
      title={open ? undefined : rest}
    >
      <span className={styles.box_glyph} aria-hidden>
        {done ? "☑" : "☐"}
      </span>
      {marker && <span className={styles.marker}>{marker}</span>}
      <span className={open ? styles.item_text_open : styles.item_text}>{rest}</span>
    </button>
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
