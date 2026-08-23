import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, GitBranch, Link2, ListTree, RefreshCw, TriangleAlert, X } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { PageShell } from "./PageShell";
import { HandoffChainModal } from "./HandoffChainModal";
import { distinctWorkspaces } from "./NewSessionForm";
import { NODE_H, NODE_W, layoutPlanForest, nodeKey } from "./planLayout";
import type { LayoutEdge, LayoutNode } from "./planLayout";
import { useDetailStore, useSessionsStore, useUIStore } from "../store";
import type { HandoffChain, PlanForest, PlanNode } from "../types";
import styles from "./PlansView.module.css";

// ── Tree helpers ─────────────────────────────────────────────────────────────

/** Pending P-tasks in this plan alone. */
function pendingOf(node: PlanNode): number {
  return Math.max(0, node.total - node.done);
}

/** Does this plan — or anything below it — still have work left? Drives both
 *  the default expansion and the 「已完成」 fold: a root whose whole subtree is
 *  finished is history, and this workspace has hundreds of those. */
function subtreeHasPending(node: PlanNode): boolean {
  return pendingOf(node) > 0 || node.children.some(subtreeHasPending);
}

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
 *  literal asterisks — the P-number, the one part you scan a tree by, is the
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

/** Every node in the forest, flat — used to resolve a selection back to its plan. */
function flatten(roots: PlanNode[], out: PlanNode[] = []): PlanNode[] {
  for (const r of roots) {
    out.push(r);
    flatten(r.children, out);
  }
  return out;
}

/** Left + right margin on `.stage`; the packer gets the width that is left. */
const STAGE_MARGIN_X = 40;

/** The connector between two boxes: a horizontal-tangent cubic, so the curve
 *  leaves the parent and meets the child flat rather than at an angle. */
function edgePath(e: LayoutEdge): string {
  const dx = Math.max(18, (e.x2 - e.x1) / 2);
  return `M ${e.x1} ${e.y1} C ${e.x1 + dx} ${e.y1}, ${e.x2 - dx} ${e.y2}, ${e.x2} ${e.y2}`;
}

// ── Root view ────────────────────────────────────────────────────────────────

/**
 * 计划树 — a workspace's whole execution chain as a left-to-right node graph:
 * the `parent`-linked plan forest, one box per plan, with each plan's handoff
 * relays folded onto the node that spawned them. Server-side join lives in
 * `claw_fleet_core::plan_forest`; this view only lays it out and draws it.
 *
 * The graph carries structure and progress only — title, done/total, a progress
 * bar, and badges. Not one line of P-task prose is on the canvas: a plan's items
 * are routinely multi-paragraph implementation notes, and rendering them inline
 * (what this view used to do) turned the whole page into a wall of text with the
 * shape of the forest lost inside it. Prose lives in the right-hand drawer,
 * behind a click, one plan at a time — and even there each item is clamped to a
 * line until you open it.
 *
 * Two default folds survive from the old list, now applied to the *graph*:
 *   1. Roots whose whole subtree is finished are off-canvas behind one chip.
 *   2. A subtree with no pending work anywhere collapses into its parent node.
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
  // Which plan the drawer is showing. Deliberately not persisted: a drawer
  // restored on boot would reopen prose nobody asked to see.
  const [selectedKey, setSelectedKey] = useState<string | null>(null);

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
  // plan that is no longer on the canvas.
  useEffect(() => setSelectedKey(null), [selectedWorkspace]);

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

  // A subtree is drawn unless the user said otherwise; the default hides
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

  // The packer needs the canvas width to know how many root trees fit on a
  // band; re-measure on every resize (opening the drawer counts).
  const canvasRef = useRef<HTMLDivElement | null>(null);
  const [canvasW, setCanvasW] = useState(0);
  useEffect(() => {
    const el = canvasRef.current;
    if (!el) return;
    const measure = () => setCanvasW(el.clientWidth);
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, [forest]);

  const layout = useMemo(
    () => layoutPlanForest(shownRoots, collapsed, Math.max(NODE_W, canvasW - STAGE_MARGIN_X)),
    [shownRoots, collapsed, canvasW],
  );

  const selected = useMemo(() => {
    if (!selectedKey) return null;
    return flatten(forest?.roots ?? []).find((n) => nodeKey(n) === selectedKey) ?? null;
  }, [selectedKey, forest]);

  const pendingPlans = useMemo(
    () => flatten(shownRoots).filter((n) => pendingOf(n) > 0).length,
    [shownRoots],
  );

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
                  <i className={styles.dot_live} />
                  {t("plans.legend_live", "有待办")}
                  <i className={styles.dot_done} />
                  {t("plans.legend_done", "已完成")}
                </span>
              </div>

              <div className={styles.canvas} ref={canvasRef}>
                <div
                  className={styles.stage}
                  style={{ width: layout.width, height: layout.height }}
                >
                  <svg
                    className={styles.edges}
                    width={layout.width}
                    height={layout.height}
                    aria-hidden
                  >
                    {layout.edges.map((e) => (
                      <path key={`${e.from}->${e.to}`} d={edgePath(e)} />
                    ))}
                  </svg>
                  {layout.nodes.map((n) => (
                    <PlanNodeBox
                      key={n.key}
                      laid={n}
                      selected={n.key === selectedKey}
                      onSelect={setSelectedKey}
                      onToggleSubtree={toggleSubtree}
                    />
                  ))}
                </div>
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

// ── One node on the canvas ───────────────────────────────────────────────────

interface NodeBoxProps {
  laid: LayoutNode;
  selected: boolean;
  onSelect: (key: string) => void;
  onToggleSubtree: (key: string, open: boolean) => void;
}

function PlanNodeBox({ laid, selected, onSelect, onToggleSubtree }: NodeBoxProps) {
  const { t } = useTranslation();
  const { node } = laid;
  const pending = pendingOf(node);
  const pct = node.total > 0 ? Math.round((node.done / node.total) * 100) : 0;

  return (
    <div
      className={styles.box_wrap}
      style={{ left: laid.x, top: laid.y, width: NODE_W, height: NODE_H }}
    >
      <button
        className={[
          styles.box,
          pending === 0 ? styles.box_done : styles.box_live,
          selected ? styles.box_selected : "",
        ].join(" ")}
        title={`${node.title || node.id}\n${node.id}${node.source ? `\n${node.source}` : ""}`}
        onClick={() => onSelect(laid.key)}
      >
        <span className={styles.box_head}>
          {node.orphanedParent ? (
            <TriangleAlert size={11} strokeWidth={2} className={styles.orphan} />
          ) : (
            node.kind === "explore" && <i className={styles.kind_dot} />
          )}
          <span className={styles.box_title}>{node.title || node.id}</span>
          {node.chains.length > 0 && (
            <span className={styles.chain_count}>
              <Link2 size={10} strokeWidth={2} />
              {node.chains.length}
            </span>
          )}
          <span className={pending > 0 ? styles.progress_live : styles.progress}>
            {node.done}/{node.total}
          </span>
        </span>
        <span className={styles.bar} aria-hidden>
          <i style={{ width: `${pct}%` }} />
        </span>
      </button>

      {laid.hasChildren && (
        <button
          className={styles.knob}
          title={
            laid.collapsed
              ? t("plans.expand_subtree", { count: laid.hiddenDescendants, defaultValue: "展开 {{count}} 个子计划" })
              : t("plans.collapse_subtree", "折叠子树")
          }
          onClick={(e) => {
            e.stopPropagation();
            onToggleSubtree(laid.key, laid.collapsed);
          }}
        >
          {laid.collapsed ? laid.hiddenDescendants : "−"}
        </button>
      )}
    </div>
  );
}

// ── Detail drawer ────────────────────────────────────────────────────────────

interface DrawerProps {
  node: PlanNode;
  doneShown: boolean;
  onToggleDone: () => void;
  onOpenChain: (chain: HandoffChain) => void;
  onClose: () => void;
}

/** The only place P-task prose is rendered: one plan, on demand. */
function PlanDrawer({ node, doneShown, onToggleDone, onOpenChain, onClose }: DrawerProps) {
  const { t } = useTranslation();
  const pendingItems = node.items.filter((i) => !i.done);
  const doneItems = node.items.filter((i) => i.done);
  // Plan titles are supposed to be one line, but plenty in the wild are a whole
  // paragraph — clamp like the items do rather than let one push the tasks off.
  const [titleOpen, setTitleOpen] = useState(false);

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
        <span className={pendingOf(node) > 0 ? styles.progress_live : styles.progress}>
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
        {pendingItems.map((item, i) => (
          <TaskLine key={`p${i}`} text={item.text} />
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
        {doneShown && doneItems.map((item, i) => <TaskLine key={`d${i}`} text={item.text} done />)}
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
function TaskLine({ text, done }: { text: string; done?: boolean }) {
  const { marker, rest } = splitMarker(text);
  const [open, setOpen] = useState(false);
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
