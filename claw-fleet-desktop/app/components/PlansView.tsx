import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import { ChevronRight, GitBranch, Link2, ListTree, RefreshCw, TriangleAlert } from "lucide-react";
import { EmptyState } from "./EmptyState";
import { PageShell } from "./PageShell";
import { HandoffChainModal } from "./HandoffChainModal";
import { distinctWorkspaces } from "./NewSessionForm";
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

// ── Root view ────────────────────────────────────────────────────────────────

/**
 * 计划树 — a workspace's whole execution chain in one tree: the `parent`-linked
 * plan forest, with each plan's handoff relays folded onto the node that spawned
 * them. Server-side join lives in `claw_fleet_core::plan_forest`; this view only
 * shapes and folds it.
 *
 * Two deliberate default folds, because the raw forest is unreadable otherwise
 * (this repo carries ~350 finished plans):
 *   1. Only trees with pending work open. Finished roots collapse behind one row.
 *   2. An open plan lists its *pending* P-tasks in full; done ones collapse to a
 *      count.
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

  const setExpanded = (id: string, open: boolean) =>
    updatePlansView({ expandOverrides: { ...expandOverrides, [id]: open } });

  const toggleDoneItems = (id: string) =>
    updatePlansView({
      doneItemsShown: doneItemsShown.includes(id)
        ? doneItemsShown.filter((x) => x !== id)
        : [...doneItemsShown, id],
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

  const renderNode = (node: PlanNode, depth: number) => (
    <PlanRow
      key={`${node.source ?? ""}:${node.id}`}
      node={node}
      depth={depth}
      expandOverrides={expandOverrides}
      doneItemsShown={doneItemsShown}
      onToggle={setExpanded}
      onToggleDoneItems={toggleDoneItems}
      onOpenChain={setOpenChain}
    />
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
        {error && <div className={styles.error}>{error}</div>}
        {!error && forest && liveRoots.length === 0 && doneRoots.length === 0 && (
          <EmptyState
            icon={<ListTree size={28} strokeWidth={1.5} />}
            title={t("plans.empty_title", "这个仓库还没有计划")}
            subtitle={t("plans.empty_sub", "用 `fleet plan create <id> --root --title \"…\"` 开一棵计划树")}
          />
        )}

        {liveRoots.map((r) => renderNode(r, 0))}

        {doneRoots.length > 0 && (
          <>
            <button
              className={styles.fold_row}
              onClick={() => updatePlansView({ showCompletedRoots: !showCompletedRoots })}
            >
              <ChevronRight
                size={13}
                strokeWidth={2}
                className={showCompletedRoots ? styles.caret_open : styles.caret}
              />
              {t("plans.completed_fold", { count: donePlanCount, defaultValue: "已完成 {{count}} 个" })}
            </button>
            {showCompletedRoots && doneRoots.map((r) => renderNode(r, 0))}
          </>
        )}

        {(forest?.unattachedChains.length ?? 0) > 0 && (
          <div className={styles.section}>
            <div className={styles.section_label}>{t("plans.unattached", "未挂靠的接力链")}</div>
            {forest?.unattachedChains.map((c) => (
              <ChainChip key={c.chainId} chain={c} onOpen={setOpenChain} />
            ))}
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

// ── One plan node ────────────────────────────────────────────────────────────

interface RowProps {
  node: PlanNode;
  depth: number;
  expandOverrides: Record<string, boolean>;
  doneItemsShown: string[];
  onToggle: (id: string, open: boolean) => void;
  onToggleDoneItems: (id: string) => void;
  onOpenChain: (chain: HandoffChain) => void;
}

function PlanRow({
  node,
  depth,
  expandOverrides,
  doneItemsShown,
  onToggle,
  onToggleDoneItems,
  onOpenChain,
}: RowProps) {
  const { t } = useTranslation();
  const pending = pendingOf(node);
  const open = expandOverrides[node.id] ?? subtreeHasPending(node);
  const pendingItems = node.items.filter((i) => !i.done);
  const doneItems = node.items.filter((i) => i.done);
  const showDone = doneItemsShown.includes(node.id);

  return (
    <div className={styles.node}>
      <button
        className={`${styles.row} ${pending === 0 ? styles.row_done : ""}`}
        style={{ paddingLeft: 8 + depth * 18 }}
        onClick={() => onToggle(node.id, !open)}
      >
        <ChevronRight size={13} strokeWidth={2} className={open ? styles.caret_open : styles.caret} />
        <span className={styles.title}>{node.title || node.id}</span>
        <span className={styles.id}>{node.id}</span>
        {node.kind === "explore" && <span className={styles.kind}>explore</span>}
        {node.orphanedParent && (
          <span className={styles.orphan} title={t("plans.orphan_tip", { parent: node.orphanedParent, defaultValue: "父计划 {{parent}} 不存在,已提升为根" })}>
            <TriangleAlert size={11} strokeWidth={2} />
            {node.orphanedParent}
          </span>
        )}
        {node.source && <span className={styles.source}>{node.source}</span>}
        <span className={styles.spacer} />
        {node.chains.length > 0 && (
          <span className={styles.chain_count}>
            <Link2 size={11} strokeWidth={2} />
            {node.chains.length}
          </span>
        )}
        <span className={pending > 0 ? styles.progress_live : styles.progress}>
          {node.done}/{node.total}
        </span>
      </button>

      {open && (
        <div className={styles.body} style={{ paddingLeft: 8 + depth * 18 + 20 }}>
          {pendingItems.map((item, i) => (
            <TaskLine key={`p${i}`} text={item.text} />
          ))}
          {doneItems.length > 0 && (
            <button className={styles.done_fold} onClick={() => onToggleDoneItems(node.id)}>
              <ChevronRight
                size={12}
                strokeWidth={2}
                className={showDone ? styles.caret_open : styles.caret}
              />
              {t("plans.done_items", { count: doneItems.length, defaultValue: "已完成 {{count}} 条" })}
            </button>
          )}
          {showDone && doneItems.map((item, i) => <TaskLine key={`d${i}`} text={item.text} done />)}
          {node.chains.map((c) => (
            <ChainChip key={c.chainId} chain={c} onOpen={onOpenChain} />
          ))}
          {node.children.map((child) => (
            <PlanRow
              key={`${child.source ?? ""}:${child.id}`}
              node={child}
              depth={depth + 1}
              expandOverrides={expandOverrides}
              doneItemsShown={doneItemsShown}
              onToggle={onToggle}
              onToggleDoneItems={onToggleDoneItems}
              onOpenChain={onOpenChain}
            />
          ))}
        </div>
      )}
    </div>
  );
}

/** One P-task line: `P3` as a badge, then its prose. */
function TaskLine({ text, done }: { text: string; done?: boolean }) {
  const { marker, rest } = splitMarker(text);
  return (
    <div className={`${styles.item} ${done ? styles.item_done : ""}`}>
      <span className={styles.box} aria-hidden>
        {done ? "☑" : "☐"}
      </span>
      {marker && <span className={styles.marker}>{marker}</span>}
      <span className={styles.item_text}>{rest}</span>
    </div>
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
