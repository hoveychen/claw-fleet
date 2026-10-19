import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import { ArrowUpToLine, Brain, Copy, FolderOpen, GraduationCap, Trash2 } from "lucide-react";
import { TextBlock } from "./blocks/TextBlock";
import { EmptyState } from "./EmptyState";
import { useAutoFlip } from "./useAutoFlip";
import { PageShell } from "./PageShell";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import { useReportStore, useUIStore } from "../store";
import { canRevealPath } from "../canReveal";
import type { ManagedLesson } from "../types";
import { repoRootPath } from "../../../shared-ts/repoPath";
import styles from "./MemoryView.module.css";
import { Presence } from "./Presence";
import {
  SkeletonCard,
  SkeletonList,
  SkeletonNumber,
  SkeletonText,
  Spinner,
  TopProgress,
} from "./loading";
import { useDelayedFlag } from "../hooks/useDelayedFlag";

// ── Types ────────────────────────────────────────────────────────────────────

interface MemoryFile {
  name: string;
  path: string;
  sizeBytes: number;
  modifiedMs: number;
  title?: string;
  hook?: string;
  memType?: string;
  description?: string;
  inIndex?: boolean;
}

type MemType = "user" | "feedback" | "project" | "reference";

const TYPE_CONFIG: Record<MemType, { short: string; cssClass: string }> = {
  user: { short: "USR", cssClass: "type_user" },
  feedback: { short: "FB", cssClass: "type_feedback" },
  project: { short: "PRJ", cssClass: "type_project" },
  reference: { short: "REF", cssClass: "type_reference" },
};

const TYPE_ORDER: MemType[] = ["user", "feedback", "project", "reference"];

function isMemType(s: string | undefined): s is MemType {
  return s === "user" || s === "feedback" || s === "project" || s === "reference";
}

export interface WorkspaceMemory {
  source: "claude-code" | "codex" | string;
  workspaceName: string;
  workspacePath: string;
  projectKey: string;
  hasClaudeMd: boolean;
  files: MemoryFile[];
}

interface MemoryEditDetail {
  type: "write" | "edit";
  content?: string;
  oldString?: string;
  newString?: string;
}

interface MemoryHistoryEntry {
  sessionId: string;
  workspaceName: string;
  timestamp: string;
  tool: string;
  detail: MemoryEditDetail;
}

type StoreSelection =
  | { kind: "file"; workspace: WorkspaceMemory; file: MemoryFile }
  | { kind: "claudeMd"; workspace: WorkspaceMemory };

type Selection = StoreSelection | { kind: "lessons" } | null;

function memorySelectionKey(selection: Exclude<Selection, null>): string {
  if (selection.kind === "lessons") return "lessons";
  if (selection.kind === "claudeMd") return `claudeMd:${selection.workspace.projectKey}`;
  return `file:${selection.workspace.projectKey}:${selection.file.name}`;
}

function resolveMemorySelection(
  key: string | null,
  workspaces: WorkspaceMemory[],
): Selection {
  if (key === "lessons") return { kind: "lessons" };
  for (const workspace of workspaces) {
    if (key === `claudeMd:${workspace.projectKey}` && workspace.hasClaudeMd) {
      return { kind: "claudeMd", workspace };
    }
    const file = workspace.files.find(
      (candidate) => key === `file:${workspace.projectKey}:${candidate.name}`,
    );
    if (file) return { kind: "file", workspace, file };
  }
  return null;
}

// ── Helpers ──────────────────────────────────────────────────────────────────

function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes}B`;
  return `${(bytes / 1024).toFixed(1)}K`;
}

function relativeTime(ms: number): string {
  if (!ms) return "";
  const diff = Date.now() - ms;
  const sec = Math.floor(diff / 1000);
  if (sec < 60) return `${sec}s ago`;
  const min = Math.floor(sec / 60);
  if (min < 60) return `${min}m ago`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr}h ago`;
  const day = Math.floor(hr / 24);
  if (day < 30) return `${day}d ago`;
  const mo = Math.floor(day / 30);
  if (mo < 12) return `${mo}mo ago`;
  return `${Math.floor(mo / 12)}y ago`;
}

function formatTime(iso: string): string {
  try {
    const d = new Date(iso);
    return d.toLocaleString(undefined, {
      month: "short",
      day: "numeric",
      hour: "2-digit",
      minute: "2-digit",
    });
  } catch {
    return iso;
  }
}

// ── Shared pieces ────────────────────────────────────────────────────────────
// The global memory page (Settings) and a project's memory tab (Projects) are
// the two places memory is shown; both are built from these.

const GLOBAL_KEY = "__global__";
const CODEX_KEY = "__codex__";

/** Stores that belong to no project: ~/.claude's own memory and CLAUDE.md, and
 *  Codex's machine-wide memory. The global memory page shows exactly these; a
 *  project's memory tab shows the stores whose workspace folds to its root. */
export function isMachineWideStore(ws: WorkspaceMemory): boolean {
  return ws.projectKey === GLOBAL_KEY || ws.projectKey === CODEX_KEY;
}

function storeLabel(t: TFunction, ws: WorkspaceMemory): string {
  return ws.projectKey === GLOBAL_KEY ? t("memory.global_label") : ws.workspaceName;
}

type PromoteTarget = "project" | "global";

/** Every memory store, refetched on `memories-updated`. A refetch while the
 *  list is already on screen keeps the old list and raises `refreshing`. */
function useMemoryStores() {
  const [memories, setMemories] = useState<WorkspaceMemory[]>([]);
  const [loaded, setLoaded] = useState(false);
  const [refreshCount, setRefreshCount] = useState(0);
  const refreshing = useDelayedFlag(refreshCount > 0);
  const trackRefresh = useCallback(async (work: () => Promise<void>) => {
    setRefreshCount((n) => n + 1);
    try {
      await work();
    } finally {
      setRefreshCount((n) => n - 1);
    }
  }, []);

  const load = useCallback(async () => {
    try {
      setMemories(await invoke<WorkspaceMemory[]>("list_memories"));
    } catch {
      // Keep whatever is on screen; an empty first load shows the empty state.
    } finally {
      setLoaded(true);
    }
  }, []);
  const reload = useCallback(() => trackRefresh(load), [trackRefresh, load]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    const unlisten = listen("memories-updated", () => void reload());
    return () => {
      unlisten.then((f) => f());
    };
  }, [reload]);

  // Same call the detail pane's promote button fires; refetches so the file
  // surfaces under its new store.
  const promote = useCallback(
    (ws: WorkspaceMemory, file: MemoryFile, target: PromoteTarget) =>
      trackRefresh(async () => {
        try {
          await invoke("promote_memory", {
            memoryPath: file.path,
            target,
            workspacePath: ws.workspacePath,
          });
          await load();
        } catch (e) {
          console.error("promote_memory failed:", e);
        }
      }),
    [trackRefresh, load],
  );

  return { memories, loaded, refreshing, reload, promote };
}

/** A memory card's right-click menu. `node` has to be rendered somewhere in the
 *  list for the menu to show. */
function useMemoryCardMenu(
  promote: (ws: WorkspaceMemory, file: MemoryFile, target: PromoteTarget) => Promise<void>,
) {
  const { t } = useTranslation();
  const [menu, setMenu] = useState<{
    ws: WorkspaceMemory;
    file: MemoryFile;
    anchor: ContextMenuAnchor;
  } | null>(null);

  const items = (ws: WorkspaceMemory, file: MemoryFile): ContextMenuItem[] => {
    const out: ContextMenuItem[] = [];
    // Promotion mirrors the detail pane: only real Claude-Code entries, never
    // the MEMORY.md index.
    if (file.name !== "MEMORY.md" && ws.source === "claude-code") {
      out.push({
        id: "promote-project",
        label: t("memory.promote_project"),
        icon: <ArrowUpToLine size={13} strokeWidth={1.7} />,
        onSelect: () => void promote(ws, file, "project"),
      });
      out.push({
        id: "promote-global",
        label: t("memory.promote_global"),
        icon: <ArrowUpToLine size={13} strokeWidth={1.7} />,
        onSelect: () => void promote(ws, file, "global"),
      });
    }
    out.push({
      id: "copy-content",
      label: t("memory.copy_content", "复制内容"),
      icon: <Copy size={13} strokeWidth={1.7} />,
      onSelect: () =>
        void invoke<string>("get_memory_content", { path: file.path })
          .then((c) => writeText(c))
          .catch(() => {}),
    });
    out.push({
      id: "copy-path",
      label: t("paths.copy_path", "复制路径"),
      icon: <Copy size={13} strokeWidth={1.7} />,
      sub: file.path,
      onSelect: () => void writeText(file.path).catch(() => {}),
    });
    // Memory files on a remote probe live on that host — nothing to reveal here.
    // Same in the browser build, where `reveal_path` is a no-op: a menu item
    // that silently does nothing is worse than an absent one. (canReveal.ts)
    if (canRevealPath()) {
      const revealKey =
        document.documentElement.getAttribute("data-platform") === "windows"
          ? "paths.reveal_in_explorer"
          : "paths.reveal_in_finder";
      out.push({
        id: "reveal",
        label: t(revealKey),
        icon: <FolderOpen size={13} strokeWidth={1.7} />,
        onSelect: () => void invoke("reveal_path", { path: file.path }).catch(() => {}),
      });
    }
    return out;
  };

  const open = (ws: WorkspaceMemory, file: MemoryFile, e: React.MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    setMenu({ ws, file, anchor: { x: e.clientX, y: e.clientY } });
  };

  const node = (
    <Presence when={Boolean(menu)}>{menu && (
      <ContextMenu
        anchor={menu.anchor}
        items={items(menu.ws, menu.file)}
        onClose={() => setMenu(null)}
      />
    )}</Presence>
  );

  return { open, node };
}

/** Which stores have their "not in MEMORY.md" fold open. UI state shared by
 *  both places memory is listed. */
function useUnindexedFolds() {
  const expandedKeys = useUIStore((s) => s.mainViewState.memory.expandedKeys);
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const expanded = useMemo(() => new Set(expandedKeys), [expandedKeys]);
  const toggle = (key: string) => {
    const next = new Set(expanded);
    if (next.has(key)) next.delete(key);
    else next.add(key);
    updateMainViewState("memory", { expandedKeys: [...next] });
  };
  return { expanded, toggle };
}

/** One store's cards: header (name, count, source, CLAUDE.md), the entries
 *  MEMORY.md indexes, then a fold of the ones it does not. */
function MemoryStoreGroup({
  ws,
  selection,
  onSelect,
  onCardMenu,
  expanded,
  onToggleUnindexed,
}: {
  ws: WorkspaceMemory;
  selection: Selection;
  onSelect: (next: StoreSelection) => void;
  onCardMenu: (ws: WorkspaceMemory, file: MemoryFile, e: React.MouseEvent) => void;
  expanded: boolean;
  onToggleUnindexed: () => void;
}) {
  const { t } = useTranslation();
  const indexed = ws.files.filter((f) => f.inIndex || f.name === "MEMORY.md");
  const unindexed = ws.files.filter((f) => !f.inIndex && f.name !== "MEMORY.md");
  const card = (f: MemoryFile) => (
    <MemoryCard
      key={f.path}
      file={f}
      source={ws.source}
      active={selection?.kind === "file" && selection.file.path === f.path}
      onClick={() => onSelect({ kind: "file", workspace: ws, file: f })}
      onContextMenu={(e) => onCardMenu(ws, f, e)}
    />
  );
  return (
    <div className={styles.workspace_group}>
      <div className={styles.workspace_header}>
        <span className={styles.workspace_name}>{storeLabel(t, ws)}</span>
        <span className={styles.workspace_count}>{ws.files.length}</span>
        <span className={styles.source_badge}>
          {ws.source === "codex" ? "Codex" : "Claude"}
        </span>
        {ws.hasClaudeMd && (
          <button
            className={`${styles.badge} ${styles.badge_claude} ${
              selection?.kind === "claudeMd" &&
              selection.workspace.projectKey === ws.projectKey
                ? styles.badge_active
                : ""
            }`}
            onClick={() => onSelect({ kind: "claudeMd", workspace: ws })}
          >
            CLAUDE.md
          </button>
        )}
      </div>
      <div className={styles.card_list}>{indexed.map(card)}</div>
      {unindexed.length > 0 && (
        <div className={styles.unindexed_section}>
          <button className={styles.unindexed_header} onClick={onToggleUnindexed}>
            <span className={styles.unindexed_caret}>{expanded ? "▼" : "▶"}</span>
            <span className={styles.unindexed_label}>{t("memory.unindexed_section")}</span>
            <span className={styles.unindexed_count}>{unindexed.length}</span>
          </button>
          {expanded && <div className={styles.card_list}>{unindexed.map(card)}</div>}
        </div>
      )}
    </div>
  );
}

/** The detail pane of a selected file or CLAUDE.md. */
function StoreDetail({
  t,
  selection,
  onPromoted,
}: {
  t: TFunction;
  selection: StoreSelection;
  onPromoted: () => Promise<void>;
}) {
  return selection.kind === "claudeMd" ? (
    <ClaudeMdDetail
      workspaceName={storeLabel(t, selection.workspace)}
      workspacePath={selection.workspace.workspacePath}
    />
  ) : (
    <FileDetail workspace={selection.workspace} file={selection.file} onPromoted={onPromoted} />
  );
}

// ── Global memory page ───────────────────────────────────────────────────────

/** 全局记忆: the machine-wide stores plus the Fleet lessons, opened from
 *  Settings. Project memory lives in each project's memory tab
 *  ({@link ProjectMemory}). */
export function MemoryView() {
  const { t } = useTranslation();
  const { memories, loaded, refreshing, reload, promote } = useMemoryStores();
  const { query, filterType, sourceFilter, selectedKey } = useUIStore(
    (s) => s.mainViewState.memory,
  );
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const setQuery = (value: string) => updateMainViewState("memory", { query: value });
  const setFilterType = (value: string) =>
    updateMainViewState("memory", { filterType: value });
  const setSourceFilter = (value: "all" | "claude-code" | "codex") =>
    updateMainViewState("memory", { sourceFilter: value });
  const setSelection = (next: Selection) =>
    updateMainViewState("memory", {
      selectedKey: next ? memorySelectionKey(next) : null,
    });
  const folds = useUnindexedFolds();
  const cardMenu = useMemoryCardMenu(async (ws, file, target) => {
    await promote(ws, file, target);
    setSelection(null);
  });

  // Lessons the user added to global guidance from the daily-report card
  // (~/.claude/fleet-lessons.md), shown as a pinned entry with per-row removal.
  const { managedLessons, loadManagedLessons, removeManagedLesson } = useReportStore();
  // The report store has no loaded flag for lessons (and swallows errors), so
  // track the first fetch here: until it settles the pinned group is a skeleton.
  const [lessonsLoaded, setLessonsLoaded] = useState(false);
  useEffect(() => {
    void loadManagedLessons().finally(() => setLessonsLoaded(true));
  }, [loadManagedLessons]);

  const stores = useMemo(() => memories.filter(isMachineWideStore), [memories]);

  // Type counts across the shown stores (excludes MEMORY.md itself).
  const typeCounts = useMemo(() => {
    const counts: Record<MemType | "all", number> = {
      all: 0,
      user: 0,
      feedback: 0,
      project: 0,
      reference: 0,
    };
    for (const ws of stores) {
      for (const f of ws.files) {
        if (f.name === "MEMORY.md") continue;
        counts.all++;
        if (isMemType(f.memType)) counts[f.memType]++;
      }
    }
    return counts;
  }, [stores]);

  const sourceCounts = useMemo(() => ({
    "claude-code": stores
      .filter((ws) => ws.source === "claude-code")
      .reduce((sum, ws) => sum + ws.files.length, 0),
    codex: stores
      .filter((ws) => ws.source === "codex")
      .reduce((sum, ws) => sum + ws.files.length, 0),
  }), [stores]);

  // Filter by query (name/title/hook/description) + by selected type + by
  // source. MEMORY.md is always kept (it's the index).
  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase();
    return stores
      .map((ws) => {
        if (sourceFilter !== "all" && ws.source !== sourceFilter) return null;
        const files = ws.files.filter((f) => {
          if (f.name === "MEMORY.md") return filterType === "all" && !q;
          if (filterType !== "all" && f.memType !== filterType) return false;
          if (!q) return true;
          const hay = [f.name, f.title ?? "", f.hook ?? "", f.description ?? ""]
            .join(" ")
            .toLowerCase();
          return hay.includes(q);
        });
        if (files.length === 0 && !(ws.hasClaudeMd && !q)) return null;
        return { ...ws, files } satisfies WorkspaceMemory;
      })
      .filter((x): x is WorkspaceMemory => x !== null);
  }, [stores, query, filterType, sourceFilter]);

  const selection = useMemo(
    () => resolveMemorySelection(selectedKey, filtered),
    [selectedKey, filtered],
  );

  // Keep selection valid after refresh / filter changes.
  useEffect(() => {
    if (loaded && selectedKey && !selection) setSelection(null);
  }, [loaded, selectedKey, selection]);

  const totalFiles = typeCounts.all;

  return (
    <PageShell
      view="memory"
      detailKey={selectedKey}
      className={styles.mem_scope}
      title={t("settings.page_global_memory", "全局记忆")}
      count={loaded && totalFiles > 0 ? totalFiles : null}
      search={{
        value: query,
        onChange: setQuery,
        placeholder: t("memory.search_placeholder"),
      }}
      subBar={
        <>
          <FilterChip
            label={t("memory.source_claude")}
            count={loaded ? sourceCounts["claude-code"] : null}
            active={sourceFilter === "claude-code"}
            onClick={() => setSourceFilter(sourceFilter === "claude-code" ? "all" : "claude-code")}
          />
          <FilterChip
            label={t("memory.source_codex")}
            count={loaded ? sourceCounts.codex : null}
            active={sourceFilter === "codex"}
            onClick={() => setSourceFilter(sourceFilter === "codex" ? "all" : "codex")}
          />
          <FilterChip
            label={t("memory.filter_all")}
            count={loaded ? typeCounts.all : null}
            active={filterType === "all"}
            onClick={() => setFilterType("all")}
          />
          {TYPE_ORDER.map((type) => (
            <FilterChip
              key={type}
              label={t(`memory.filter_${type}`)}
              count={loaded ? typeCounts[type] : null}
              active={filterType === type}
              typeClass={TYPE_CONFIG[type].cssClass}
              onClick={() => setFilterType(type)}
            />
          ))}
        </>
      }
      secondary={
        <div className={`${styles.list_pane} ${styles.list_pane_positioned}`}>
          <TopProgress active={refreshing} />
          {!loaded && <SkeletonList rows={10} avatar />}
          {loaded && !lessonsLoaded && <SkeletonList rows={1} avatar />}
          {loaded && lessonsLoaded && filtered.length === 0 && managedLessons.length === 0 && (
            <EmptyState
              icon={<Brain size={28} strokeWidth={1.5} />}
              title={t("empty_state.memory_title")}
              subtitle={t("empty_state.memory_subtitle")}
            />
          )}
          {managedLessons.length > 0 && (
            <div className={styles.workspace_group}>
              <div className={styles.workspace_header}>
                <span className={styles.workspace_name}>
                  {t("memory.lessons_group")}
                </span>
                <span className={styles.workspace_count}>
                  {managedLessons.length}
                </span>
              </div>
              <div className={styles.card_list}>
                <button
                  className={`${styles.card} ${
                    selection?.kind === "lessons" ? styles.card_active : ""
                  }`}
                  onClick={() => setSelection({ kind: "lessons" })}
                >
                  <span className={`${styles.type_badge} ${styles.type_index}`}>
                    <GraduationCap size={13} strokeWidth={2} />
                  </span>
                  <div className={styles.card_body}>
                    <div className={styles.card_title}>
                      {t("memory.lessons_entry_title")}
                    </div>
                    <div className={styles.card_hook}>
                      {t("memory.lessons_entry_hook")}
                    </div>
                  </div>
                </button>
              </div>
            </div>
          )}
          {filtered.map((ws) => (
            <MemoryStoreGroup
              key={ws.projectKey}
              ws={ws}
              selection={selection}
              onSelect={setSelection}
              onCardMenu={cardMenu.open}
              expanded={folds.expanded.has(ws.projectKey)}
              onToggleUnindexed={() => folds.toggle(ws.projectKey)}
            />
          ))}
          {cardMenu.node}
        </div>
      }
    >
      {selection === null && !loaded ? (
        <div className={styles.detail_body}>
          <SkeletonCard height={160} />
        </div>
      ) : selection === null ? (
        <div className={styles.placeholder}>
          {totalFiles > 0
            ? t("settings.page_global_memory", "全局记忆")
            : t("memory.no_memories")}
        </div>
      ) : selection.kind === "lessons" ? (
        <ManagedLessonsDetail
          lessons={managedLessons}
          onRemove={removeManagedLesson}
        />
      ) : (
        <StoreDetail
          t={t}
          selection={selection}
          onPromoted={async () => {
            setSelection(null);
            await reload();
          }}
        />
      )}
    </PageShell>
  );
}

// ── A project's memory tab ───────────────────────────────────────────────────

/** The memory of one project, read and managed in place: every store whose
 *  workspace folds to `projectPath` (a repo's subdirectories each get their own
 *  Claude store) on the left, the picked entry on the right. */
export function ProjectMemory({ projectPath }: { projectPath: string }) {
  const { t } = useTranslation();
  const { memories, loaded, refreshing, reload, promote } = useMemoryStores();
  const [selectedKey, setSelectedKey] = useState<string | null>(null);
  const setSelection = (next: Selection) =>
    setSelectedKey(next ? memorySelectionKey(next) : null);
  const folds = useUnindexedFolds();
  const cardMenu = useMemoryCardMenu(async (ws, file, target) => {
    await promote(ws, file, target);
    setSelection(null);
  });

  const stores = useMemo(
    () =>
      memories.filter(
        (ws) => !isMachineWideStore(ws) && repoRootPath(ws.workspacePath) === projectPath,
      ),
    [memories, projectPath],
  );

  // Open on the first entry, in list order, rather than an empty right half.
  const firstKey = useMemo(() => {
    const ws = stores[0];
    if (!ws) return null;
    if (ws.hasClaudeMd) return memorySelectionKey({ kind: "claudeMd", workspace: ws });
    const file = ws.files.find((f) => f.inIndex || f.name === "MEMORY.md") ?? ws.files[0];
    return file ? memorySelectionKey({ kind: "file", workspace: ws, file }) : null;
  }, [stores]);
  const selection =
    resolveMemorySelection(selectedKey, stores) ?? resolveMemorySelection(firstKey, stores);

  if (!loaded) return <SkeletonList rows={5} />;
  if (stores.length === 0) {
    return (
      <EmptyState
        icon={<Brain size={28} strokeWidth={1.5} />}
        title={t("projects.memory_empty", "这个项目还没有记忆")}
        subtitle={t("projects.memory_empty_sub", "agent 在这里干活时记下的要点会出现在这里。")}
      />
    );
  }
  return (
    <div className={`${styles.mem_scope} ${styles.project_split}`}>
      <div className={`${styles.list_pane} ${styles.list_pane_positioned} ${styles.project_list}`}>
        <TopProgress active={refreshing} />
        {stores.map((ws) => (
          <MemoryStoreGroup
            key={ws.projectKey}
            ws={ws}
            selection={selection}
            onSelect={setSelection}
            onCardMenu={cardMenu.open}
            expanded={folds.expanded.has(ws.projectKey)}
            onToggleUnindexed={() => folds.toggle(ws.projectKey)}
          />
        ))}
        {cardMenu.node}
      </div>
      <div className={styles.project_detail}>
        {selection && selection.kind !== "lessons" && (
          <StoreDetail
            t={t}
            selection={selection}
            onPromoted={async () => {
              setSelection(null);
              await reload();
            }}
          />
        )}
      </div>
    </div>
  );
}

// ── File detail pane ─────────────────────────────────────────────────────────

function FileDetail({
  workspace,
  file,
  onPromoted,
}: {
  workspace: WorkspaceMemory;
  file: MemoryFile;
  onPromoted: () => Promise<void>;
}) {
  const { t } = useTranslation();
  const tab = useUIStore((s) => s.mainViewState.memory.detailTab);
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const setTab = (detailTab: "content" | "history") =>
    updateMainViewState("memory", { detailTab });
  const [content, setContent] = useState<string | null>(null);
  const [history, setHistory] = useState<MemoryHistoryEntry[] | null>(null);
  const [loadingContent, setLoadingContent] = useState(true);
  const [loadingHistory, setLoadingHistory] = useState(false);
  const [promoting, setPromoting] = useState(false);
  const [showPromoteMenu, setShowPromoteMenu] = useState(false);
  const promoteMenuRef = useRef<HTMLDivElement | null>(null);
  const promoteSide = useAutoFlip(showPromoteMenu, "below", promoteMenuRef);

  useEffect(() => {
    setContent(null);
    setHistory(null);
    setShowPromoteMenu(false);
  }, [file.path]);

  useEffect(() => {
    setLoadingContent(true);
    invoke<string>("get_memory_content", { path: file.path })
      .then(setContent)
      .catch(() => setContent("(error reading file)"))
      .finally(() => setLoadingContent(false));
  }, [file.path]);

  useEffect(() => {
    if (tab === "history" && history === null) {
      setLoadingHistory(true);
      invoke<MemoryHistoryEntry[]>("get_memory_history", { path: file.path })
        .then(setHistory)
        .catch(() => setHistory([]))
        .finally(() => setLoadingHistory(false));
    }
  }, [tab, file.path, history]);

  const handlePromote = async (target: "project" | "global") => {
    if (promoting) return;
    setPromoting(true);
    setShowPromoteMenu(false);
    try {
      await invoke("promote_memory", {
        memoryPath: file.path,
        target,
        workspacePath: workspace.workspacePath,
      });
      await onPromoted();
    } catch (e) {
      console.error("promote_memory failed:", e);
      setPromoting(false);
    }
  };

  const isIndex = file.name === "MEMORY.md";

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          {workspace.projectKey === "__global__"
            ? t("memory.global_label")
            : workspace.workspaceName}
          <span className={styles.detail_sep}>/</span>
          <span className={styles.detail_name}>{file.name}</span>
        </div>
        <div className={styles.detail_actions}>
          {!isIndex && workspace.source === "claude-code" && (
            <div className={styles.promote_wrapper}>
              <button
                className={styles.promote_btn}
                disabled={promoting}
                onClick={() => setShowPromoteMenu((v) => !v)}
              >
                {promoting && <Spinner size={12} className={styles.btn_spinner} />}
                {promoting ? t("memory.promoting") : t("memory.promote")}
              </button>
              <Presence when={Boolean(showPromoteMenu)}>{showPromoteMenu && (
                <div
                  className={`${styles.promote_menu} ${
                    promoteSide === "above" ? styles.promote_menu_above : styles.promote_menu_below
                  }`}
                  ref={promoteMenuRef}
                >
                  <button
                    className={styles.promote_menu_item}
                    onClick={() => handlePromote("project")}
                  >
                    {t("memory.promote_project")}
                  </button>
                  <button
                    className={styles.promote_menu_item}
                    onClick={() => handlePromote("global")}
                  >
                    {t("memory.promote_global")}
                  </button>
                </div>
              )}</Presence>
            </div>
          )}
        </div>
      </div>

      <div className={styles.tabs}>
        <button
          className={`${styles.tab} ${tab === "content" ? styles.tab_active : ""}`}
          onClick={() => setTab("content")}
        >
          {t("memory.tab_content")}
        </button>
        {workspace.source === "claude-code" && (
          <button
            className={`${styles.tab} ${tab === "history" ? styles.tab_active : ""}`}
            onClick={() => setTab("history")}
          >
            {t("memory.tab_history")}
            {history && ` (${history.length})`}
          </button>
        )}
      </div>

      <div className={styles.detail_body}>
        {tab === "content" && (
          <>
            {loadingContent && content === null && <SkeletonText lines={10} />}
            {content !== null && (
              <div className={styles.content_markdown}>
                <TextBlock text={content} />
              </div>
            )}
          </>
        )}

        {tab === "history" && (
          <>
            {loadingHistory && history === null && <SkeletonList rows={4} meta />}
            {history !== null && history.length === 0 && (
              <p className={styles.no_history}>{t("memory.no_history")}</p>
            )}
            {history && history.length > 0 && (
              <div className={styles.history_list}>
                {history.map((entry, i) => (
                  <div key={i} className={styles.history_entry}>
                    <div className={styles.history_header}>
                      <span
                        className={`${styles.history_tool} ${
                          entry.tool === "Write"
                            ? styles.history_tool_write
                            : styles.history_tool_edit
                        }`}
                      >
                        {entry.tool}
                      </span>
                      <span className={styles.history_time}>
                        {formatTime(entry.timestamp)}
                      </span>
                      <span className={styles.history_session}>
                        {entry.sessionId.slice(0, 8)}
                      </span>
                    </div>
                    <div className={styles.history_body}>
                      {entry.detail.type === "write" && entry.detail.content}
                      {entry.detail.type === "edit" && (
                        <>
                          <span className={styles.diff_old}>
                            - {entry.detail.oldString}
                          </span>
                          <span className={styles.diff_new}>
                            + {entry.detail.newString}
                          </span>
                        </>
                      )}
                    </div>
                  </div>
                ))}
              </div>
            )}
          </>
        )}
      </div>
    </>
  );
}

// ── CLAUDE.md detail pane ────────────────────────────────────────────────────

function ClaudeMdDetail({
  workspaceName,
  workspacePath,
}: {
  workspaceName: string;
  workspacePath: string;
}) {
  const [content, setContent] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    setLoading(true);
    setContent(null);
    invoke<string>("get_claude_md_content", { workspacePath })
      .then(setContent)
      .catch(() => setContent("(error reading CLAUDE.md)"))
      .finally(() => setLoading(false));
  }, [workspacePath]);

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          {workspaceName}
          <span className={styles.detail_sep}>/</span>
          <span className={styles.detail_name}>CLAUDE.md</span>
        </div>
      </div>
      <div className={styles.detail_body}>
        {loading && <SkeletonText lines={10} />}
        {content !== null && (
          <div className={styles.content_markdown}>
            <TextBlock text={content} />
          </div>
        )}
      </div>
    </>
  );
}

// ── Managed lessons detail pane ──────────────────────────────────────────────

function ManagedLessonsDetail({
  lessons,
  onRemove,
}: {
  lessons: ManagedLesson[];
  onRemove: (id: string) => Promise<void>;
}) {
  const { t } = useTranslation();
  const [removing, setRemoving] = useState<Set<string>>(new Set());

  const handleRemove = async (id: string) => {
    if (removing.has(id)) return;
    setRemoving((prev) => new Set(prev).add(id));
    try {
      await onRemove(id);
    } finally {
      setRemoving((prev) => {
        const next = new Set(prev);
        next.delete(id);
        return next;
      });
    }
  };

  return (
    <>
      <div className={styles.detail_header}>
        <div className={styles.detail_title}>
          <span className={styles.detail_name}>{t("memory.lessons_entry_title")}</span>
        </div>
      </div>
      <div className={styles.detail_body}>
        {lessons.length === 0 ? (
          <p className={styles.no_history}>{t("memory.lessons_empty")}</p>
        ) : (
          <div className={styles.lesson_list}>
            {lessons.map((l) => (
              <div key={l.id} className={styles.lesson_card}>
                <div className={styles.lesson_content}>
                  <TextBlock text={l.content} />
                </div>
                {l.reason && (
                  <div className={styles.lesson_why}>
                    <span className={styles.lesson_why_label}>{t("memory.lessons_why")}</span>
                    <div className={styles.lesson_why_body}>
                      <TextBlock text={l.reason} />
                    </div>
                  </div>
                )}
                <div className={styles.lesson_footer}>
                  <div className={styles.lesson_meta}>
                    <span className={styles.lesson_workspace}>{l.workspaceName}</span>
                    <span className={styles.card_meta_dot}>·</span>
                    <span className={styles.lesson_session}>{l.sessionId}</span>
                  </div>
                  <button
                    className={styles.lesson_remove}
                    disabled={removing.has(l.id)}
                    onClick={() => handleRemove(l.id)}
                  >
                    {removing.has(l.id) ? <Spinner size={12} /> : <Trash2 size={12} strokeWidth={2} />}
                    {removing.has(l.id) ? t("memory.lessons_removing") : t("memory.lessons_remove")}
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </div>
    </>
  );
}

// ── Filter chip ──────────────────────────────────────────────────────────────

function FilterChip({
  label,
  count,
  active,
  typeClass,
  onClick,
}: {
  label: string;
  /** null while the list has not loaded yet: render a skeleton, not a fake 0. */
  count: number | null;
  active: boolean;
  typeClass?: string;
  onClick: () => void;
}) {
  return (
    <button
      className={`${styles.chip} ${active ? styles.chip_active : ""} ${
        typeClass ? styles[typeClass] ?? "" : ""
      }`}
      onClick={onClick}
    >
      <span className={styles.chip_label}>{label}</span>
      <span className={styles.chip_count}>
        {count === null ? <SkeletonNumber width={10} /> : count}
      </span>
    </button>
  );
}

// ── Memory card (single entry in the list) ───────────────────────────────────

function MemoryCard({
  file,
  source,
  active,
  onClick,
  onContextMenu,
}: {
  file: MemoryFile;
  source: string;
  active: boolean;
  onClick: () => void;
  onContextMenu: (e: React.MouseEvent) => void;
}) {
  const isIndexFile = file.name === "MEMORY.md";
  const memType = isMemType(file.memType) ? file.memType : null;
  const rawTitle = file.title ?? file.name;
  // MEMORY.md uses filename-as-title when not curated; strip the .md suffix
  // so cards don't show e.g. "feedback_ddb_fp_artifacts.md" as the title.
  const title = rawTitle === file.name ? rawTitle.replace(/\.md$/, "") : rawTitle;
  const subtitle = file.hook ?? file.description ?? "";
  return (
    <button
      className={`${styles.card} ${active ? styles.card_active : ""} ${
        isIndexFile ? styles.card_index : ""
      }`}
      onClick={onClick}
      onContextMenu={onContextMenu}
    >
      {source === "codex" ? (
        <span className={`${styles.type_badge} ${styles.type_codex}`}>CX</span>
      ) : isIndexFile ? (
        <span className={`${styles.type_badge} ${styles.type_index}`}>IDX</span>
      ) : memType ? (
        <span
          className={`${styles.type_badge} ${styles[TYPE_CONFIG[memType].cssClass]}`}
        >
          {TYPE_CONFIG[memType].short}
        </span>
      ) : (
        <span className={`${styles.type_badge} ${styles.type_unknown}`}>?</span>
      )}
      <div className={styles.card_body}>
        <div className={styles.card_title}>{title}</div>
        {subtitle && <div className={styles.card_hook}>{subtitle}</div>}
        <div className={styles.card_meta}>
          <span>{formatBytes(file.sizeBytes)}</span>
          {file.modifiedMs > 0 && (
            <>
              <span className={styles.card_meta_dot}>·</span>
              <span>{relativeTime(file.modifiedMs)}</span>
            </>
          )}
        </div>
      </div>
    </button>
  );
}
