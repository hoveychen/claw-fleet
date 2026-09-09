import { invoke } from "@tauri-apps/api/core";
import { open as openDialog, save } from "@tauri-apps/plugin-dialog";
import {
  Archive,
  ChevronDown,
  ChevronRight,
  ChevronUp,
  CheckCheck,
  Download,
  FileArchive,
  FileSpreadsheet,
  FileText,
  FileType,
  Film,
  Folder,
  FolderOpen,
  FolderInput,
  FolderPlus,
  History,
  Image as ImageIcon,
  LayoutGrid,
  Music,
  Package,
  Pencil,
  Presentation,
  Rows3,
  Star,
  Trash2,
  TriangleAlert,
  X,
} from "lucide-react";
import { lazy, Suspense, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { artifactBlobUrl } from "../artifactAssets";
import { canRevealPath } from "../canReveal";
import { formatBytes } from "../formatBytes";
import { isWebBuild } from "../hostEnv";
import { getItem, setItem } from "../storage";
import { useUIStore } from "../store";
import { dropTargetAt, usePointerDrag } from "../hooks/usePointerDrag";
import { officeMode, textPreviewMode, thumbMode } from "../officePreview";
import { downloadArtifact, downloadFolderZip } from "../mock/liveProxy";
import { isBrowsableArchive } from "../../../shared-ts/zipDir";
import { PageShell } from "./PageShell";
import { ZipBrowser } from "./ZipBrowser";
import { EmptyState } from "./EmptyState";
import { TextBlock } from "./blocks/TextBlock";
import styles from "./ArtifactsView.module.css";

/** Mirrors `claw_fleet_core::artifacts::Artifact`. */
export interface Artifact {
  id: string;
  name: string;
  title: string;
  note: string;
  mime: string;
  kind: string;
  sizeBytes: number;
  createdMs: number;
  workspacePath: string;
  workspaceName: string;
  /**
   * The folder the user filed this in, `/`-separated, `""` when unfiled.
   *
   * Empty falls back to a folder derived from `sourcePath` — see
   * {@link artifactRelativeDirectory}. That fallback is what keeps every
   * artifact ingested before folders existed in the place it always appeared.
   */
  path: string;
  sessionId: string | null;
  sourcePath: string;
  starred: boolean;
  hardlinked: boolean;
  drifted: boolean;
  /** Which entry of `versions` the fields above describe. */
  currentVersion: string;
  /** Every ingest of this deliverable, newest first — always at least one. */
  versions: ArtifactVersion[];
}

/** Mirrors `claw_fleet_core::artifacts::ArtifactVersion`. */
export interface ArtifactVersion {
  id: string;
  addedMs: number;
  sizeBytes: number;
  sourcePath: string;
  hardlinked: boolean;
}

/** Mirrors `claw_fleet_core::artifacts::FolderZip`. */
export interface FolderZip {
  filename: string;
  memberCount: number;
  totalBytes: number;
  /** Artifacts whose stored bytes were missing, by title. */
  skipped: string[];
}

/** Mirrors `claw_fleet_core::artifacts::Folder`. */
export interface ArtifactFolder {
  workspacePath: string;
  path: string;
}

interface StoreUsage {
  count: number;
  totalBytes: number;
  hardlinkedBytes: number;
}

export type SortKey = "recent" | "size" | "name" | "workspace";
export type SortDir = "asc" | "desc";

/**
 * Which way each key reads first.
 *
 * Times and sizes want the big end first ("what did I just make", "what is
 * eating the disk"); names and workspaces want A→Z. A single global default
 * would make one of the two groups useless on the first click.
 */
export const DEFAULT_SORT_DIR: Record<SortKey, SortDir> = {
  recent: "desc",
  size: "desc",
  name: "asc",
  workspace: "asc",
};

/** Grid of thumbnails, or a dense sortable table. */
export type ArtifactLayout = "grid" | "list";

export interface ArtifactDirectoryNode {
  key: string;
  label: string;
  workspacePath: string;
  directory: string;
  count: number;
  children: ArtifactDirectoryNode[];
}

const KIND_ICON: Record<string, typeof FileText> = {
  image: ImageIcon,
  video: Film,
  audio: Music,
  pdf: FileType,
  doc: FileText,
  sheet: FileSpreadsheet,
  slides: Presentation,
  archive: Archive,
  text: FileText,
};

/**
 * The three Office renderers, kept out of the main bundle.
 *
 * Together they are ~1.6 MB of JavaScript (pptx-preview bundles echarts for a
 * deck's native charts), which no session should download to look at a session
 * list. `lazy` defers the module, and the module defers each library again —
 * see OfficePreview's own docs.
 */
const OfficePreview = lazy(() => import("./OfficePreview"));

/** Same libraries, same reason to defer them — see `ArtifactThumb`. */
const ArtifactThumb = lazy(() => import("./ArtifactThumb"));

export { formatBytes };

/**
 * Order artifacts.
 *
 * Its own exported function because the ordering is the part worth testing:
 * "newest first" has to survive same-millisecond ids (two `fleet artifact add`
 * calls in one script), and the name sort has to be locale-aware or a CJK
 * title lands in a random position.
 *
 * `dir` exists for the list view's column headers, where clicking the same
 * column again has to reverse it. Omitted, every key keeps the direction the
 * sub-bar's dropdown has always implied — newest and biggest first, names
 * A→Z — so the grid is unaffected.
 */
export function sortArtifacts(list: Artifact[], key: SortKey, dir?: SortDir): Artifact[] {
  const out = [...list];
  const flip = dir && dir !== DEFAULT_SORT_DIR[key] ? -1 : 1;
  switch (key) {
    case "size":
      return out.sort((a, b) => flip * (b.sizeBytes - a.sizeBytes || a.id.localeCompare(b.id)));
    case "name":
      return out.sort(
        (a, b) => flip * a.title.localeCompare(b.title, undefined, { numeric: true }),
      );
    case "workspace":
      return out.sort(
        (a, b) =>
          flip *
          (a.workspaceName.localeCompare(b.workspaceName, undefined, { numeric: true }) ||
            // Within one workspace, the folder is the next meaningful level.
            (artifactRelativeDirectory(a) ?? "").localeCompare(
              artifactRelativeDirectory(b) ?? "",
              undefined,
              { numeric: true },
            ) ||
            b.createdMs - a.createdMs),
      );
    case "recent":
    default:
      // Ids are timestamps with a collision suffix, so they break a createdMs
      // tie in the same direction the store's own listing does.
      return out.sort((a, b) => flip * (b.createdMs - a.createdMs || b.id.localeCompare(a.id)));
  }
}

/** Apply the sub-bar's filters. Exported for the same reason as the sort. */
export function filterArtifacts(
  list: Artifact[],
  opts: { query: string; workspace: string; directory?: string; starredOnly: boolean },
): Artifact[] {
  const q = opts.query.trim().toLowerCase();
  return list.filter((a) => {
    if (opts.starredOnly && !a.starred) return false;
    if (opts.workspace && a.workspacePath !== opts.workspace) return false;
    if (opts.directory) {
      const directory = artifactRelativeDirectory(a);
      if (directory !== opts.directory && !directory?.startsWith(`${opts.directory}/`)) return false;
    }
    if (!q) return true;
    // Note and filename included on purpose: the title is often the filename,
    // and what the user remembers is as likely to be "the one about Q3".
    return (
      a.title.toLowerCase().includes(q) ||
      a.name.toLowerCase().includes(q) ||
      a.note.toLowerCase().includes(q)
    );
  });
}

/**
 * Filenames for a batch export, de-duplicated.
 *
 * Two artifacts can carry the same original filename — `report.pdf` from two
 * different sessions is the normal case, not a corner one — and exporting them
 * into one directory would silently leave only the second. Suffix the repeats
 * the way a browser's download folder does, before the extension so the file
 * still opens.
 */
export function uniqueExportNames(names: string[]): string[] {
  const used = new Set<string>();
  return names.map((name) => {
    if (!used.has(name)) {
      used.add(name);
      return name;
    }
    const dot = name.lastIndexOf(".");
    const stem = dot > 0 ? name.slice(0, dot) : name;
    const ext = dot > 0 ? name.slice(dot) : "";
    for (let n = 2; ; n += 1) {
      const candidate = `${stem} (${n})${ext}`;
      if (!used.has(candidate)) {
        used.add(candidate);
        return candidate;
      }
    }
  });
}

/** Join a picked directory with a filename, keeping the platform's separator. */
export function joinExportPath(dir: string, name: string): string {
  const sep = dir.includes("\\") && !dir.includes("/") ? "\\" : "/";
  return `${dir.replace(/[/\\]+$/, "")}${sep}${name}`;
}

/**
 * What a press on `id` drags.
 *
 * A press on something already checked drags the whole selection — that is
 * what makes "tick five, drag them together" work. A press on anything else
 * drags just that one and leaves the selection alone, so an accidental drag
 * can never move files the user had forgotten were ticked.
 *
 * Its own pure function because this is the rule worth pinning: the two
 * branches differ in how many files a single gesture moves, and getting it
 * backwards would move things silently.
 */
export function dragSet(
  shown: Artifact[],
  checked: ReadonlySet<string>,
  id: string,
): Artifact[] {
  if (checked.has(id)) return shown.filter((a) => checked.has(a.id));
  const one = shown.find((a) => a.id === id);
  return one ? [one] : [];
}

/**
 * A folder row's drop-zone key, and how to read one back.
 *
 * The key carries the workspace as well as the path because a folder only
 * exists *within* a workspace — the tree's top level is the workspace, and
 * "交付" under repo A is a different place from "交付" under repo B. Dropping
 * across workspaces is refused (see `dropTargetFolder`) rather than silently
 * re-homing a deliverable to a repo it did not come from.
 *
 * Same `data-` attribute mechanism the wiki's folder rail uses: `dropTargetAt`
 * hit-tests for the nearest ancestor carrying it, so a nested row naturally
 * wins over the workspace row it sits inside.
 */
export const DROP_ATTR = "data-artifact-drop";

export function dropKey(workspacePath: string, directory: string): string {
  return `${workspacePath}\u0000${directory}`;
}

/**
 * Where a drop landed, or `null` when it landed nowhere it may go.
 *
 * `null` for: outside any folder row, and — deliberately — a row belonging to
 * a different workspace than the dragged artifacts. Mixed selections spanning
 * two workspaces therefore cannot be dropped at all, which is the honest
 * outcome: there is no single destination that means the same thing for both.
 */
export function dropTargetFolder(
  key: string | null,
  dragging: { workspacePath: string }[],
): { workspacePath: string; directory: string } | null {
  if (key === null || dragging.length === 0) return null;
  const cut = key.indexOf("\u0000");
  if (cut < 0) return null;
  const workspacePath = key.slice(0, cut);
  const directory = key.slice(cut + "\u0000".length);
  if (dragging.some((a) => a.workspacePath !== workspacePath)) return null;
  return { workspacePath, directory };
}

/**
 * What the selection becomes after a click on `id`.
 *
 * `order` is the list as displayed, which is what makes a shift-click mean
 * "everything between these two rows *on screen*" rather than "between these
 * two ids" — the same click has to select a different set depending on how the
 * list is sorted, so the ordering has to come in from the caller.
 *
 * Shift extends from the anchor and only ever *adds*: a shift-click that
 * silently deselected what you already had checked would be a data-loss
 * gesture right next to a 批量删除 button.
 */
export function nextSelection(
  current: ReadonlySet<string>,
  order: string[],
  id: string,
  opts: { shift: boolean; anchor: string | null },
): { selected: Set<string>; anchor: string | null } {
  const out = new Set(current);
  const from = opts.anchor === null ? -1 : order.indexOf(opts.anchor);
  const to = order.indexOf(id);
  if (opts.shift && from >= 0 && to >= 0) {
    for (let i = Math.min(from, to); i <= Math.max(from, to); i += 1) out.add(order[i]);
    // The anchor stays put, so a second shift-click re-extends from the same
    // origin instead of walking it forward one row at a time.
    return { selected: out, anchor: opts.anchor };
  }
  if (out.has(id)) {
    out.delete(id);
    // Deselecting the anchor would leave a shift-click extending from a row
    // that is no longer checked.
    return { selected: out, anchor: opts.anchor === id ? null : opts.anchor };
  }
  out.add(id);
  return { selected: out, anchor: id };
}

function normalizeArtifactPath(path: string): string {
  return path.replaceAll("\\", "/").replace(/\/+$/, "");
}

/**
 * Which folder an artifact shows up in.
 *
 * `path` — what the user filed it as — wins. Only an unfiled artifact falls
 * back to deriving a folder from where the producing agent happened to write
 * the file, which is all this page had before folders were user-owned; without
 * the fallback every existing artifact would jump to the workspace root the
 * day the field shipped.
 *
 * `null` means "no idea": unfiled *and* produced outside its own workspace.
 */
export function artifactRelativeDirectory(artifact: Artifact): string | null {
  if (artifact.path) return artifact.path;
  const workspace = normalizeArtifactPath(artifact.workspacePath);
  const source = normalizeArtifactPath(artifact.sourcePath);
  const workspaceLower = workspace.toLowerCase();
  const sourceLower = source.toLowerCase();
  if (!sourceLower.startsWith(`${workspaceLower}/`)) return null;
  const relative = source.slice(workspace.length + 1);
  const slash = relative.lastIndexOf("/");
  return slash < 0 ? "" : relative.slice(0, slash);
}

interface MutableDirectoryNode extends Omit<ArtifactDirectoryNode, "children"> {
  children: MutableDirectoryNode[];
  childMap: Map<string, MutableDirectoryNode>;
}

/**
 * Build the secondary navigation.
 *
 * Two inputs, because a folder has to be able to exist before anything is in
 * it: the artifacts contribute the folders they are filed in (and the counts),
 * `folders` contributes the ones the user made and has not filled yet. Walking
 * artifacts alone would make a freshly created folder vanish until the first
 * drop, which reads as the button not having worked.
 */
export function buildArtifactDirectoryTree(
  items: Artifact[],
  folders: ArtifactFolder[] = [],
): ArtifactDirectoryNode[] {
  const roots = new Map<string, MutableDirectoryNode>();
  // A workspace's display name only appears on its artifacts, so collect it up
  // front for the folders' sake: an empty folder in an otherwise artifact-less
  // workspace would be labelled with a raw absolute path without this.
  const names = new Map<string, string>();
  for (const artifact of items) {
    if (artifact.workspaceName) names.set(artifact.workspacePath, artifact.workspaceName);
  }

  const ensureRoot = (workspacePath: string): MutableDirectoryNode => {
    let root = roots.get(workspacePath);
    if (!root) {
      root = {
        key: workspacePath,
        label: names.get(workspacePath) || workspacePath,
        workspacePath,
        directory: "",
        count: 0,
        children: [],
        childMap: new Map(),
      };
      roots.set(workspacePath, root);
    }
    return root;
  };

  /** Walk to `directory`, creating the levels that don't exist yet. */
  const descend = (root: MutableDirectoryNode, directory: string, counts: boolean) => {
    let current = root;
    const parts = directory.split("/").filter(Boolean);
    for (let index = 0; index < parts.length; index += 1) {
      const path = parts.slice(0, index + 1).join("/");
      let child = current.childMap.get(parts[index]);
      if (!child) {
        child = {
          key: `${root.workspacePath}\u0000${path}`,
          label: parts[index],
          workspacePath: root.workspacePath,
          directory: path,
          count: 0,
          children: [],
          childMap: new Map(),
        };
        current.childMap.set(parts[index], child);
        current.children.push(child);
      }
      if (counts) child.count += 1;
      current = child;
    }
  };

  for (const artifact of items) {
    const root = ensureRoot(artifact.workspacePath);
    root.count += 1;
    const directory = artifactRelativeDirectory(artifact);
    if (!directory) continue;
    descend(root, directory, true);
  }

  // The folders the user made: they add nodes, never counts.
  for (const folder of folders) {
    if (!folder.path) continue;
    descend(ensureRoot(folder.workspacePath), folder.path, false);
  }
  const finalize = (node: MutableDirectoryNode): ArtifactDirectoryNode => ({
    key: node.key,
    label: node.label,
    workspacePath: node.workspacePath,
    directory: node.directory,
    count: node.count,
    children: node.children
      .sort((left, right) => left.label.localeCompare(right.label, undefined, { numeric: true }))
      .map(finalize),
  });
  return [...roots.values()]
    .sort((left, right) => left.label.localeCompare(right.label, undefined, { numeric: true }))
    .map(finalize);
}

export function ArtifactsView() {
  const { t } = useTranslation();
  const [items, setItems] = useState<Artifact[] | null>(null);
  const [folders, setFolders] = useState<ArtifactFolder[]>([]);
  const [usage, setUsage] = useState<StoreUsage | null>(null);
  const [query, setQuery] = useState("");
  const [workspace, setWorkspace] = useState("");
  const [directory, setDirectory] = useState("");
  const [starredOnly, setStarredOnly] = useState(false);
  // Layout and ordering come off the persisted store, so the page opens the way
  // it was left. `getItem` is a synchronous read of the cache `initStorage()`
  // filled before render, so this is safe as a `useState` initializer.
  const [layout, setLayout] = useState<ArtifactLayout>(
    () => (getItem("artifacts-layout") === "list" ? "list" : "grid"),
  );
  const [sortKey, setSortKey] = useState<SortKey>(() => {
    const stored = getItem("artifacts-sort-key");
    return stored && stored in DEFAULT_SORT_DIR ? (stored as SortKey) : "recent";
  });
  const [sortDir, setSortDir] = useState<SortDir>(() =>
    getItem("artifacts-sort-dir") === "asc" ? "asc" : "desc",
  );
  const [selectedId, setSelectedId] = useState<string | null>(null);
  // The batch selection, separate from `selectedId` (which is "the one whose
  // detail pane is open"). Two different questions, two different states.
  const [checked, setChecked] = useState<ReadonlySet<string>>(() => new Set());
  const [anchorId, setAnchorId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const chooseLayout = useCallback((next: ArtifactLayout) => {
    setLayout(next);
    setItem("artifacts-layout", next);
  }, []);

  /**
   * Sort by `key`; clicking the column already sorted reverses it.
   *
   * The first click on a *new* column uses that key's natural direction rather
   * than inheriting the previous column's — sorting by name and getting Z→A
   * because you were last on "newest first" reads as a bug.
   */
  const chooseSort = useCallback(
    (key: SortKey) => {
      const next: SortDir =
        key === sortKey
          ? sortDir === "asc"
            ? "desc"
            : "asc"
          : DEFAULT_SORT_DIR[key];
      setSortKey(key);
      setSortDir(next);
      setItem("artifacts-sort-key", key);
      setItem("artifacts-sort-dir", next);
    },
    [sortKey, sortDir],
  );

  const load = useCallback(async () => {
    // `?? []` rather than the raw result: the mock's `default:` branch answers
    // null, and a null here would blank the page on the first filter() — the
    // exact failure MOCK_WIKI_DOCS exists to prevent for the wiki.
    const list = (await invoke<Artifact[]>("list_artifacts").catch(() => [])) ?? [];
    setItems(list);
    setFolders((await invoke<ArtifactFolder[]>("list_artifact_folders").catch(() => [])) ?? []);
    setUsage((await invoke<StoreUsage>("artifact_usage").catch(() => null)) ?? null);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // "Open this deliverable", raised by a transcript's ingest card. The stage is
  // driven by `selectedId` alone — no filter has to be cleared, because the
  // detail replaces the grid rather than living inside it. The request is only
  // consumed once the artifact is actually in `items`: a card clicked seconds
  // after the ingest can arrive before this page's first load returns.
  const artifactNav = useUIStore((s) => s.artifactNav);
  const clearArtifactNav = useUIStore((s) => s.clearArtifactNav);
  useEffect(() => {
    if (!artifactNav) return;
    if (!(items ?? []).some((a) => a.id === artifactNav.id)) return;
    setSelectedId(artifactNav.id);
    clearArtifactNav();
  }, [artifactNav, items, clearArtifactNav]);

  const directoryTree = useMemo(
    () => buildArtifactDirectoryTree(items ?? [], folders),
    [items, folders],
  );

  const shown = useMemo(
    () =>
      sortArtifacts(
        filterArtifacts(items ?? [], { query, workspace, directory, starredOnly }),
        sortKey,
        sortDir,
      ),
    [items, query, workspace, directory, starredOnly, sortKey, sortDir],
  );

  const selected = useMemo(
    () => (items ?? []).find((a) => a.id === selectedId) ?? null,
    [items, selectedId],
  );

  /**
   * Drop anything checked that is no longer on screen.
   *
   * Otherwise narrowing the filter and hitting 批量删除 would delete artifacts
   * the user can't see — the checkbox count would say 5 while the list showed
   * 2. Keyed on the visible ids so it also survives a reload that removed one.
   */
  const shownIds = useMemo(() => shown.map((a) => a.id), [shown]);
  useEffect(() => {
    setChecked((prev) => {
      if (prev.size === 0) return prev;
      const visible = new Set(shownIds);
      const next = new Set([...prev].filter((id) => visible.has(id)));
      return next.size === prev.size ? prev : next;
    });
  }, [shownIds]);

  const toggleChecked = useCallback(
    (id: string, shift: boolean) => {
      const { selected: next, anchor } = nextSelection(checked, shownIds, id, {
        shift,
        anchor: anchorId,
      });
      setChecked(next);
      setAnchorId(anchor);
    },
    [checked, shownIds, anchorId],
  );

  const clearChecked = useCallback(() => {
    setChecked(new Set());
    setAnchorId(null);
  }, []);

  const checkedItems = useMemo(() => shown.filter((a) => checked.has(a.id)), [shown, checked]);

  const runBatch = useCallback(
    async (items: Artifact[], step: (artifact: Artifact, index: number) => Promise<void>) => {
      setBusy(true);
      const failed: string[] = [];
      for (const [index, artifact] of items.entries()) {
        try {
          await step(artifact, index);
        } catch (e) {
          failed.push(`${artifact.title}: ${String(e)}`);
        }
      }
      setBusy(false);
      setError(
        failed.length === 0
          ? null
          : t("artifacts.batch_failed", "{{count}} 份失败：{{detail}}", {
              count: failed.length,
              detail: failed.join("；"),
            }),
      );
      return failed.length;
    },
    [t],
  );

  const batchDelete = useCallback(async () => {
    const items = checkedItems;
    if (
      !window.confirm(
        t("artifacts.batch_delete_confirm", "删除选中的 {{count}} 份产出？此操作不可撤销。", {
          count: items.length,
        }),
      )
    ) {
      return;
    }
    await runBatch(items, (a) => invoke("delete_artifact", { id: a.id }));
    clearChecked();
    await load();
  }, [checkedItems, runBatch, clearChecked, load, t]);

  const batchMove = useCallback(async () => {
    const items = checkedItems;
    // Everything in one batch shares a destination, so one prompt. The path is
    // normalized (and refused) server-side, so a typo comes back as an error
    // rather than creating a folder named "  交付 / ".
    const target = window.prompt(
      t("artifacts.batch_move_prompt", "把选中的 {{count}} 份移动到哪个文件夹？（留空＝工作区根目录）", {
        count: items.length,
      }),
      items[0]?.path ?? "",
    );
    if (target === null) return;
    await runBatch(items, async (a) => {
      await invoke<Artifact>("update_artifact", { id: a.id, path: target });
    });
    clearChecked();
    await load();
  }, [checkedItems, runBatch, clearChecked, load, t]);

  const batchExport = useCallback(async () => {
    const items = checkedItems;
    const names = uniqueExportNames(items.map((a) => a.name));
    // A tab cannot be handed a destination directory, so it falls back to the
    // browser's own download folder, one file at a time — same split the
    // single-artifact 导出 already makes.
    if (isWebBuild()) {
      await runBatch(items, (a, i) => downloadArtifact(a.id, names[i]));
      return;
    }
    const dir = await openDialog({ multiple: false, directory: true });
    if (typeof dir !== "string") return;
    await runBatch(items, (a, i) =>
      invoke("export_artifact", { id: a.id, dest: joinExportPath(dir, names[i]) }),
    );
  }, [checkedItems, runBatch]);

  const patch = useCallback(
    async (
      id: string,
      fields: { title?: string; note?: string; starred?: boolean; path?: string },
    ) => {
      try {
        const updated = await invoke<Artifact>("update_artifact", { id, ...fields });
        setItems((prev) => (prev ?? []).map((a) => (a.id === id ? updated : a)));
        setError(null);
      } catch (e) {
        setError(String(e));
      }
    },
    [],
  );

  /**
   * Run a folder mutation, then reload.
   *
   * A full reload rather than a local splice: renaming a folder re-files every
   * artifact under it server-side, so the artifact list is stale too and
   * patching only the folder array would leave the counts wrong.
   */
  const folderOp = useCallback(
    async (op: () => Promise<unknown>) => {
      try {
        await op();
        setError(null);
        await load();
      } catch (e) {
        setError(String(e));
      }
    },
    [load],
  );

  const createFolder = useCallback(
    (workspacePath: string, path: string) =>
      folderOp(() => invoke("create_artifact_folder", { workspacePath, path })),
    [folderOp],
  );

  const renameFolder = useCallback(
    (workspacePath: string, from: string, to: string) =>
      folderOp(() => invoke("rename_artifact_folder", { workspacePath, from, to })),
    [folderOp],
  );

  /**
   * Pack a folder (recursively) into a zip the user picks a location for.
   *
   * The plan is fetched first so an empty folder is refused before a save
   * dialog appears, and so the dialog can propose a real filename. In the
   * browser build there is no dialog to show — the server streams the archive
   * as a download instead, which `downloadFolderZip` handles.
   */
  const exportFolder = useCallback(
    async (workspacePath: string, directory: string) => {
      try {
        const plan = await invoke<FolderZip>("artifact_folder_zip_plan", {
          workspacePath,
          directory,
        });
        if (plan.memberCount === 0) {
          setError(t("artifacts.zip_empty", "这个文件夹里没有产出，无需打包。"));
          return;
        }
        if (isWebBuild()) {
          await downloadFolderZip(workspacePath, directory, plan.filename);
          setError(null);
          return;
        }
        const dest = await save({ defaultPath: plan.filename });
        if (!dest) return;
        setBusy(true);
        const done = await invoke<FolderZip>("export_artifact_folder", {
          workspacePath,
          directory,
          dest,
        });
        setError(
          done.skipped.length === 0
            ? null
            : t("artifacts.zip_skipped", "已打包 {{count}} 份，跳过 {{skipped}}（存储的字节已不在）", {
                count: done.memberCount,
                skipped: done.skipped.join("、"),
              }),
        );
      } catch (e) {
        setError(t("artifacts.zip_failed", "打包失败：{{error}}", { error: String(e) }));
      } finally {
        setBusy(false);
      }
    },
    [t],
  );

  const deleteFolder = useCallback(
    (workspacePath: string, path: string) =>
      folderOp(async () => {
        await invoke("delete_artifact_folder", { workspacePath, path });
        // The rail's selection may have just been deleted from under it.
        if (workspace === workspacePath && directory.startsWith(path)) {
          setDirectory("");
        }
      }),
    [folderOp, workspace, directory],
  );

  /** Folder paths offered when filing an artifact, for one workspace. */
  // ── Drag to file into a folder ──────────────────────────────────────────
  //
  // Pointer-based, not HTML5 DnD — the latter is inert in this webview (see
  // `usePointerDrag`). One hook for the whole view; which card was pressed
  // travels in a ref that each card's pointerdown stamps.
  const pressedId = useRef<string | null>(null);
  const [dragging, setDragging] = useState<Artifact[]>([]);
  const [dropAt, setDropAt] = useState<string | null>(null);
  const [dragPoint, setDragPoint] = useState<{ x: number; y: number } | null>(null);

  const dragSetFor = useCallback(
    (id: string): Artifact[] => dragSet(shown, checked, id),
    [checked, shown],
  );

  const endDrag = useCallback(() => {
    pressedId.current = null;
    setDragging([]);
    setDropAt(null);
    setDragPoint(null);
  }, []);

  const fileInto = useCallback(
    async (items: Artifact[], directory: string) => {
      const moved = items.filter((a) => a.path !== directory);
      if (moved.length === 0) return;
      await runBatch(moved, async (a) => {
        await invoke<Artifact>("update_artifact", { id: a.id, path: directory });
      });
      clearChecked();
      await load();
    },
    [runBatch, clearChecked, load],
  );

  const cardDrag = usePointerDrag({
    onStart: (p) => {
      const id = pressedId.current;
      if (!id) return false;
      const set = dragSetFor(id);
      if (set.length === 0) return false;
      setDragging(set);
      setDragPoint({ x: p.x, y: p.y });
    },
    onMove: (p) => {
      setDragPoint({ x: p.x, y: p.y });
      setDropAt(dropTargetAt(p.over, DROP_ATTR));
    },
    onDrop: (p) => {
      const items = dragging.length > 0 ? dragging : dragSetFor(pressedId.current ?? "");
      const target = dropTargetFolder(dropTargetAt(p.over, DROP_ATTR), items);
      endDrag();
      if (target) void fileInto(items, target.directory);
    },
    onCancel: endDrag,
  });

  /** Props every draggable row/card spreads. */
  const dragProps = useCallback(
    (id: string) => ({
      onPointerDown: (e: React.PointerEvent) => {
        pressedId.current = id;
        cardDrag.onPointerDown(e);
      },
    }),
    [cardDrag],
  );
  const [busy, setBusy] = useState(false);

  /**
   * Run `step` for every checked artifact, collecting failures instead of
   * stopping at the first one.
   *
   * Aborting halfway through a batch of 20 leaves the user with no idea which
   * ones landed. Every item is attempted; the ones that failed are named in one
   * error line at the end, and the successes stand.
   */
  const folderOptions = useCallback(
    (workspacePath: string): string[] => {
      const out = new Set<string>();
      for (const f of folders) {
        if (f.workspacePath === workspacePath && f.path) out.add(f.path);
      }
      // Paths already in use count as folders even without a record — a
      // pre-folders artifact's derived directory is a real place to file into.
      for (const a of items ?? []) {
        if (a.workspacePath !== workspacePath) continue;
        const dir = artifactRelativeDirectory(a);
        if (dir) out.add(dir);
      }
      return [...out].sort((left, right) => left.localeCompare(right, undefined, { numeric: true }));
    },
    [folders, items],
  );

  /**
   * The sub-bar while something is checked.
   *
   * It *replaces* the filter bar rather than sitting beside it: a batch action
   * applies to the current selection, and leaving the filter chips live next to
   * it invites changing the visible set with a delete button already aimed.
   */
  const selectionBar = (
    /* One row, icon verbs.
     *
     * This sub-bar lives in the ~260px middle column, where five labelled
     * chips wrapped onto three lines. Icons with a `title` keep the whole
     * batch vocabulary on one line and leave the count — the thing you check
     * before pressing 删除 — as the only text. */
    <div className={styles.selection_bar}>
      <span className={styles.selection_count}>
        {t("artifacts.selected_n_short", "{{count}} 份 · {{size}}", {
          count: checkedItems.length,
          size: formatBytes(checkedItems.reduce((sum, a) => sum + a.sizeBytes, 0)),
        })}
      </span>
      <span className={styles.selection_actions}>
        <button
          type="button"
          className={styles.selection_action}
          title={t("artifacts.select_all", "全选当前")}
          aria-label={t("artifacts.select_all", "全选当前")}
          onClick={() => {
            setChecked(new Set(shownIds));
            setAnchorId(shownIds[shownIds.length - 1] ?? null);
          }}
        >
          <CheckCheck size={14} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className={styles.selection_action}
          title={t("artifacts.batch_move", "移动到…")}
          aria-label={t("artifacts.batch_move", "移动到…")}
          disabled={busy}
          onClick={batchMove}
        >
          <FolderInput size={14} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className={styles.selection_action}
          title={t("artifacts.batch_export", "导出到…")}
          aria-label={t("artifacts.batch_export", "导出到…")}
          disabled={busy}
          onClick={batchExport}
        >
          <Download size={14} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className={`${styles.selection_action} ${styles.selection_action_danger}`}
          title={t("artifacts.batch_delete", "删除")}
          aria-label={t("artifacts.batch_delete", "删除")}
          disabled={busy}
          onClick={batchDelete}
        >
          <Trash2 size={14} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className={styles.selection_action}
          title={t("artifacts.select_none", "清空选择")}
          aria-label={t("artifacts.select_none", "清空选择")}
          onClick={clearChecked}
        >
          <X size={14} strokeWidth={1.5} />
        </button>
      </span>
    </div>
  );

  const subBar = (
    <div className={styles.filters}>
      <button
        className={`${styles.chip} ${!starredOnly ? styles.chip_on : ""}`}
        onClick={() => setStarredOnly(false)}
      >
        {t("artifacts.filter_all", "全部")}
      </button>
      <button
        className={`${styles.chip} ${starredOnly ? styles.chip_on : ""}`}
        onClick={() => setStarredOnly(true)}
      >
        {t("artifacts.filter_starred", "已收藏")}
      </button>
      <select
        className={styles.select}
        value={sortKey}
        onChange={(e) => chooseSort(e.target.value as SortKey)}
        aria-label={t("artifacts.sort_by", "排序方式")}
      >
        <option value="recent">{t("artifacts.sort_recent", "最近加入")}</option>
        <option value="size">{t("artifacts.sort_size", "大小")}</option>
        <option value="name">{t("artifacts.sort_name", "名称")}</option>
        <option value="workspace">{t("artifacts.sort_workspace", "来源")}</option>
      </select>
      <span className={styles.layout_switch}>
        <button
          type="button"
          className={`${styles.layout_button} ${layout === "grid" ? styles.layout_button_on : ""}`}
          title={t("artifacts.layout_grid", "网格视图")}
          aria-label={t("artifacts.layout_grid", "网格视图")}
          aria-pressed={layout === "grid"}
          onClick={() => chooseLayout("grid")}
        >
          <LayoutGrid size={14} strokeWidth={1.5} />
        </button>
        <button
          type="button"
          className={`${styles.layout_button} ${layout === "list" ? styles.layout_button_on : ""}`}
          title={t("artifacts.layout_list", "列表视图")}
          aria-label={t("artifacts.layout_list", "列表视图")}
          aria-pressed={layout === "list"}
          onClick={() => chooseLayout("list")}
        >
          <Rows3 size={14} strokeWidth={1.5} />
        </button>
      </span>
      {usage && usage.count > 0 && (
        <span className={styles.usage}>
          {t("artifacts.usage", "{{count}} 份 · 共 {{size}}", {
            count: usage.count,
            size: formatBytes(usage.totalBytes),
          })}
        </span>
      )}
    </div>
  );

  return (
    <PageShell
      view="artifacts"
      title={t("artifacts.panel_title", "产出")}
      count={items?.length ?? null}
      search={{
        value: query,
        onChange: setQuery,
        placeholder: t("artifacts.search_placeholder", "搜索产出…"),
      }}
      subBar={selected ? undefined : checkedItems.length > 0 ? selectionBar : subBar}
      secondary={
        <ArtifactDirectoryTree
          nodes={directoryTree}
          dropKeyActive={dropAt}
          selectedKey={workspace ? `${workspace}\u0000${directory}` : ""}
          totalCount={items?.length ?? 0}
          onSelect={(nextWorkspace, nextDirectory) => {
            setWorkspace(nextWorkspace);
            setDirectory(nextDirectory);
            setSelectedId(null);
          }}
          onCreateFolder={createFolder}
          onRenameFolder={renameFolder}
          onDeleteFolder={deleteFolder}
          onExportFolder={exportFolder}
        />
      }
    >
      {error && <div className={styles.error_line}>{error}</div>}
      {/* What the pointer is carrying. `pointer-events: none` in CSS, or it
          would hit-test as the drop target under itself. */}
      {dragging.length > 0 && dragPoint && (
        <div
          className={styles.drag_ghost}
          style={{ left: dragPoint.x + 12, top: dragPoint.y + 12 }}
        >
          {dragging.length === 1
            ? dragging[0].title
            : t("artifacts.drag_n", "{{count}} 份产出", { count: dragging.length })}
        </div>
      )}
      {selected ? (
        <ArtifactDetail
          artifact={selected}
          folderOptions={folderOptions(selected.workspacePath)}
          onBack={() => setSelectedId(null)}
          onPatch={patch}
          onDeleted={async () => {
            setSelectedId(null);
            await load();
          }}
          // A rollback rewrites the artifact's current version, size and
          // source, so the list has to be refetched — but the detail pane
          // stays open on the same card.
          onReloaded={load}
          onError={setError}
        />
      ) : items === null ? (
        <EmptyState icon={<Package size={30} strokeWidth={1.1} />} title={t("artifacts.loading", "加载中…")} />
      ) : shown.length === 0 ? (
        <EmptyState
          icon={<Package size={30} strokeWidth={1.1} />}
          title={items.length === 0
            ? t("artifacts.empty_title", "还没有产出")
            : t("artifacts.empty_directory", "这个目录里没有匹配的产出")}
          subtitle={t(
            items.length === 0 ? "artifacts.empty_subtitle" : "artifacts.empty_directory_hint",
            "Agent 用 `fleet artifact add <path>` 把交付物存进来。",
          )}
        />
      ) : layout === "list" ? (
        <ArtifactTable
          items={shown}
          sortKey={sortKey}
          sortDir={sortDir}
          checked={checked}
          draggingIds={dragging.map((d) => d.id)}
          dragProps={dragProps}
          onSort={chooseSort}
          onOpen={(id) => {
            if (cardDrag.didDrag()) return;
            setSelectedId(id);
          }}
          onToggleChecked={toggleChecked}
          onToggleStar={(a) => patch(a.id, { starred: !a.starred })}
        />
      ) : (
        <div className={styles.grid}>
          {shown.map((a) => (
            <ArtifactCard
              key={a.id}
              artifact={a}
              checked={checked.has(a.id)}
              dragging={dragging.some((d) => d.id === a.id)}
              dragProps={dragProps(a.id)}
              // A completed drag ends in a click on the card it started from;
              // without this guard every drop would also open the detail pane.
              onOpen={() => {
                if (cardDrag.didDrag()) return;
                setSelectedId(a.id);
              }}
              onToggleChecked={(shift) => toggleChecked(a.id, shift)}
              onToggleStar={() => patch(a.id, { starred: !a.starred })}
            />
          ))}
        </div>
      )}
    </PageShell>
  );
}

/**
 * The dense view: one row per artifact, sortable columns.
 *
 * A real table rather than a flex grid of rows, so a screen reader announces
 * "row 3 of 40, 大小 1.4 MB" and the column headers carry `aria-sort`. The
 * thumbnail grid is for recognising a deliverable by sight; this is for the
 * jobs where you are comparing across many of them (what is biggest, what came
 * from which repo) and a 280px card per item shows five at a time.
 */
function ArtifactTable({
  items,
  sortKey,
  sortDir,
  checked,
  draggingIds,
  dragProps,
  onSort,
  onOpen,
  onToggleChecked,
  onToggleStar,
}: {
  items: Artifact[];
  sortKey: SortKey;
  sortDir: SortDir;
  checked: ReadonlySet<string>;
  draggingIds: string[];
  dragProps: (id: string) => { onPointerDown: (e: React.PointerEvent) => void };
  onSort: (key: SortKey) => void;
  onOpen: (id: string) => void;
  onToggleChecked: (id: string, shift: boolean) => void;
  onToggleStar: (artifact: Artifact) => void;
}) {
  const { t } = useTranslation();

  const header = (key: SortKey, label: string, className?: string) => (
    <th
      className={className}
      aria-sort={sortKey === key ? (sortDir === "asc" ? "ascending" : "descending") : "none"}
    >
      <button type="button" className={styles.col_button} onClick={() => onSort(key)}>
        <span>{label}</span>
        {sortKey === key &&
          (sortDir === "asc" ? <ChevronUp size={12} /> : <ChevronDown size={12} />)}
      </button>
    </th>
  );

  return (
    <div className={styles.table_pane}>
      <table className={styles.table}>
        <thead>
          <tr>
            <th className={styles.col_check} />
            <th className={styles.col_star} />
            {header("name", t("artifacts.col_name", "名称"))}
            {header("workspace", t("artifacts.col_source", "来源"), styles.col_source)}
            {header("size", t("artifacts.col_size", "大小"), styles.col_size)}
            {header("recent", t("artifacts.col_added", "加入时间"), styles.col_added)}
          </tr>
        </thead>
        <tbody>
          {items.map((a) => {
            const Icon = KIND_ICON[a.kind] ?? FileText;
            const folder = artifactRelativeDirectory(a);
            return (
              <tr
                key={a.id}
                onClick={() => onOpen(a.id)}
                className={`${styles.row} ${checked.has(a.id) ? styles.row_checked : ""} ${
                  draggingIds.includes(a.id) ? styles.row_dragging : ""
                }`}
                {...dragProps(a.id)}
              >
                <td className={styles.col_check}>
                  <input
                    type="checkbox"
                    className={styles.check}
                    checked={checked.has(a.id)}
                    aria-label={t("artifacts.select_one", "选择「{{title}}」", { title: a.title })}
                    onClick={(e) => {
                      // Stop the row's own handler, or every checkbox click
                      // also opens the detail pane.
                      e.stopPropagation();
                      onToggleChecked(a.id, e.shiftKey);
                    }}
                    // React warns about a checked input with no onChange even
                    // when the click handler is what drives it.
                    onChange={() => {}}
                  />
                </td>
                <td className={styles.col_star}>
                  <button
                    type="button"
                    className={styles.row_star}
                    aria-label={
                      a.starred ? t("artifacts.unstar", "取消收藏") : t("artifacts.star", "收藏")
                    }
                    aria-pressed={a.starred}
                    // Or the click also opens the detail pane behind it.
                    onClick={(e) => {
                      e.stopPropagation();
                      onToggleStar(a);
                    }}
                  >
                    <Star
                      size={13}
                      strokeWidth={1.5}
                      fill={a.starred ? "currentColor" : "none"}
                      className={a.starred ? styles.row_star_on : ""}
                    />
                  </button>
                </td>
                <td>
                  <span className={styles.row_name}>
                    <Icon size={14} strokeWidth={1.4} />
                    <span className={styles.row_title} title={a.name}>
                      {a.title}
                    </span>
                    {a.drifted && (
                      <TriangleAlert
                        size={12}
                        className={styles.row_drift}
                        aria-label={t("artifacts.drifted", "源文件已被改写")}
                      />
                    )}
                  </span>
                </td>
                <td className={styles.col_source}>
                  <span className={styles.row_source} title={a.workspacePath}>
                    {a.workspaceName}
                    {folder ? <span className={styles.row_folder}>/{folder}</span> : null}
                  </span>
                </td>
                <td className={styles.col_size}>{formatBytes(a.sizeBytes)}</td>
                <td className={styles.col_added}>{new Date(a.createdMs).toLocaleString()}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
    </div>
  );
}

function ArtifactDirectoryTree({
  nodes,
  selectedKey,
  dropKeyActive,
  totalCount,
  onSelect,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
  onExportFolder,
}: {
  nodes: ArtifactDirectoryNode[];
  selectedKey: string;
  /** Drop zone the pointer is currently over, or null. */
  dropKeyActive: string | null;
  totalCount: number;
  onSelect: (workspacePath: string, directory: string) => void;
  onCreateFolder: (workspacePath: string, path: string) => void;
  onRenameFolder: (workspacePath: string, from: string, to: string) => void;
  onDeleteFolder: (workspacePath: string, path: string) => void;
  onExportFolder: (workspacePath: string, path: string) => void;
}) {
  const { t } = useTranslation();
  return (
    <nav className={styles.tree} aria-label={t("artifacts.directory_tree", "产出目录")}>
      <button
        type="button"
        className={`${styles.tree_row} ${selectedKey === "" ? styles.tree_row_active : ""}`}
        onClick={() => onSelect("", "")}
      >
        <span className={styles.tree_spacer} />
        <Package size={15} strokeWidth={1.4} />
        <span className={styles.tree_label}>{t("artifacts.all_artifacts", "全部产出")}</span>
        <span className={styles.tree_count}>{totalCount}</span>
      </button>
      {nodes.map((node) => (
        <ArtifactDirectoryBranch
          key={node.key}
          node={node}
          depth={0}
          selectedKey={selectedKey}
          dropKeyActive={dropKeyActive}
          onSelect={onSelect}
          onCreateFolder={onCreateFolder}
          onRenameFolder={onRenameFolder}
          onDeleteFolder={onDeleteFolder}
          onExportFolder={onExportFolder}
        />
      ))}
    </nav>
  );
}

function ArtifactDirectoryBranch({
  node,
  depth,
  selectedKey,
  dropKeyActive,
  onSelect,
  onCreateFolder,
  onRenameFolder,
  onDeleteFolder,
  onExportFolder,
}: {
  node: ArtifactDirectoryNode;
  depth: number;
  selectedKey: string;
  dropKeyActive: string | null;
  onSelect: (workspacePath: string, directory: string) => void;
  onCreateFolder: (workspacePath: string, path: string) => void;
  onRenameFolder: (workspacePath: string, from: string, to: string) => void;
  onDeleteFolder: (workspacePath: string, path: string) => void;
  onExportFolder: (workspacePath: string, path: string) => void;
}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
  /**
   * Which inline editor this row is showing, if any.
   *
   * Inline rather than a dialog on purpose: `window.prompt` is not reliably
   * available in a Tauri webview, and a second window is exactly what the
   * desktop is moving away from.
   */
  const [editing, setEditing] = useState<null | "create" | "rename">(null);
  const myDropKey = dropKey(node.workspacePath, node.directory);
  const hasChildren = node.children.length > 0;
  const selected = selectedKey === `${node.workspacePath}\u0000${node.directory}`;
  // The workspace row is the drive, not a folder: it can hold new folders but
  // cannot itself be renamed or deleted.
  const isWorkspaceRoot = node.directory === "";

  const commit = (value: string) => {
    const name = value.trim();
    setEditing(null);
    if (!name) return;
    if (editing === "create") {
      onCreateFolder(node.workspacePath, node.directory ? `${node.directory}/${name}` : name);
      setExpanded(true);
      return;
    }
    // Rename replaces the last segment only — moving a folder elsewhere is a
    // different gesture, and typing a `/` here would silently re-nest it.
    if (name === node.label) return;
    const parent = node.directory.split("/").slice(0, -1).join("/");
    onRenameFolder(node.workspacePath, node.directory, parent ? `${parent}/${name}` : name);
  };

  return (
    <div>
      <div
        className={`${styles.tree_row} ${selected ? styles.tree_row_active : ""} ${
          dropKeyActive === myDropKey ? styles.tree_row_drop : ""
        }`}
        style={{ paddingLeft: 12 + depth * 15 }}
        // The drop zone is the whole row, including the workspace row — where
        // dropping means "out of any folder", the gesture for unfiling.
        {...{ [DROP_ATTR]: myDropKey }}
      >
        <button
          type="button"
          className={styles.tree_twisty}
          aria-label={expanded ? "Collapse" : "Expand"}
          onClick={() => setExpanded((value) => !value)}
          disabled={!hasChildren}
        >
          {hasChildren ? (expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />) : null}
        </button>
        <button type="button" className={styles.tree_target} onClick={() => onSelect(node.workspacePath, node.directory)}>
          {expanded && hasChildren ? <FolderOpen size={15} strokeWidth={1.4} /> : <Folder size={15} strokeWidth={1.4} />}
          <span className={styles.tree_label} title={node.label}>{node.label}</span>
          <span className={styles.tree_count}>{node.count}</span>
        </button>
        <span className={styles.tree_actions}>
          {/* Available on the workspace row too, where it packs everything in
              that workspace — the tree already treats that row as a folder. */}
          <button
            type="button"
            className={styles.tree_action}
            title={t("artifacts.folder_zip", "打包导出")}
            aria-label={t("artifacts.folder_zip", "打包导出")}
            onClick={() => onExportFolder(node.workspacePath, node.directory)}
          >
            <FileArchive size={13} strokeWidth={1.5} />
          </button>
          <button
            type="button"
            className={styles.tree_action}
            title={t("artifacts.folder_new", "新建文件夹")}
            aria-label={t("artifacts.folder_new", "新建文件夹")}
            onClick={() => setEditing("create")}
          >
            <FolderPlus size={13} strokeWidth={1.5} />
          </button>
          {!isWorkspaceRoot && (
            <>
              <button
                type="button"
                className={styles.tree_action}
                title={t("artifacts.folder_rename", "重命名")}
                aria-label={t("artifacts.folder_rename", "重命名")}
                onClick={() => setEditing("rename")}
              >
                <Pencil size={13} strokeWidth={1.5} />
              </button>
              <button
                type="button"
                className={styles.tree_action}
                title={t("artifacts.folder_delete", "删除文件夹")}
                aria-label={t("artifacts.folder_delete", "删除文件夹")}
                onClick={() => {
                  // Core refuses a folder that still holds anything, so this
                  // confirm is about the folder itself, not its contents.
                  if (!window.confirm(t("artifacts.folder_delete_confirm", "删除文件夹「{{name}}」？", { name: node.label }))) {
                    return;
                  }
                  onDeleteFolder(node.workspacePath, node.directory);
                }}
              >
                <Trash2 size={13} strokeWidth={1.5} />
              </button>
            </>
          )}
        </span>
      </div>
      {editing && (
        <input
          className={styles.tree_input}
          style={{ marginLeft: 32 + depth * 15 }}
          autoFocus
          defaultValue={editing === "rename" ? node.label : ""}
          placeholder={t("artifacts.folder_name_placeholder", "文件夹名")}
          onKeyDown={(e) => {
            if (e.key === "Enter") commit(e.currentTarget.value);
            if (e.key === "Escape") setEditing(null);
          }}
          // Blur commits too: clicking away from a half-typed name and having
          // it silently discarded is the more annoying of the two failures.
          onBlur={(e) => commit(e.currentTarget.value)}
        />
      )}
      {expanded && node.children.map((child) => (
        <ArtifactDirectoryBranch
          key={child.key}
          node={child}
          depth={depth + 1}
          selectedKey={selectedKey}
          dropKeyActive={dropKeyActive}
          onSelect={onSelect}
          onCreateFolder={onCreateFolder}
          onRenameFolder={onRenameFolder}
          onDeleteFolder={onDeleteFolder}
          onExportFolder={onExportFolder}
        />
      ))}
    </div>
  );
}

function ArtifactCard({
  artifact,
  checked,
  dragging,
  dragProps,
  onOpen,
  onToggleChecked,
  onToggleStar,
}: {
  artifact: Artifact;
  checked: boolean;
  /** Part of the set currently being dragged — dimmed so the pointer's cargo
   *  is visible in the grid it came from. */
  dragging: boolean;
  dragProps: { onPointerDown: (e: React.PointerEvent) => void };
  onOpen: () => void;
  onToggleChecked: (shift: boolean) => void;
  onToggleStar: () => void;
}) {
  const { t } = useTranslation();
  const Icon = KIND_ICON[artifact.kind] ?? FileText;
  // A thumbnail that fails to render leaves the icon in place rather than a
  // blank well — decoration must never make the grid worse than it was.
  const [thumbFailed, setThumbFailed] = useState(false);
  const thumb = thumbFailed ? null : thumbMode(artifact.mime, artifact.sizeBytes);
  const onThumbFail = useCallback(() => setThumbFailed(true), []);
  return (
    <div
      className={`${styles.card} ${checked ? styles.card_checked : ""} ${dragging ? styles.card_dragging : ""}`}
      onClick={onOpen}
      role="button"
      tabIndex={0}
      {...dragProps}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onOpen();
        }
      }}
    >
      {/* Visible on hover, or whenever it is checked — an invisible checked box
          would make the selection count unaccountable. */}
      <input
        type="checkbox"
        className={`${styles.card_check} ${checked ? styles.card_check_on : ""}`}
        checked={checked}
        aria-label={t("artifacts.select_one", "选择「{{title}}」", { title: artifact.title })}
        onClick={(e) => {
          e.stopPropagation();
          onToggleChecked(e.shiftKey);
        }}
        onChange={() => {}}
      />
      <div className={styles.thumb}>
        {artifact.kind === "image" ? (
          <img src={artifactBlobUrl(artifact.id, artifact.name)} alt={artifact.title} />
        ) : (
          <>
            {/* The icon stays mounted underneath: it is what shows while the
                document parses, and what remains if it never does. */}
            <Icon size={32} strokeWidth={1.2} className={styles.thumb_icon} />
            {thumb && (
              <Suspense fallback={null}>
                <ArtifactThumb
                  id={artifact.id}
                  url={artifactBlobUrl(artifact.id, artifact.name)}
                  mode={thumb}
                  title={artifact.title}
                  sizeBytes={artifact.sizeBytes}
                  onFail={onThumbFail}
                />
              </Suspense>
            )}
          </>
        )}
        <span className={styles.kind_tag}>
          {t(`artifacts.kind.${artifact.kind}`, artifact.kind)}
        </span>
        <button
          className={`${styles.star_btn} ${artifact.starred ? styles.star_on : ""}`}
          title={t(artifact.starred ? "artifacts.unstar" : "artifacts.star", "收藏")}
          onClick={(e) => {
            e.stopPropagation();
            onToggleStar();
          }}
        >
          <Star size={13} strokeWidth={1.6} fill={artifact.starred ? "currentColor" : "none"} />
        </button>
      </div>
      <div className={styles.card_body}>
        <div className={styles.card_title} title={artifact.title}>
          {artifact.title}
        </div>
        <div className={styles.card_meta}>
          <span>{formatBytes(artifact.sizeBytes)}</span>
          <span className={styles.ws_chip} title={artifact.workspacePath}>
            {artifact.workspaceName}
          </span>
          {artifact.drifted && (
            <TriangleAlert
              size={11}
              className={styles.drift_flag}
              aria-label={t("artifacts.drifted", "源文件已被改写")}
            />
          )}
        </div>
      </div>
    </div>
  );
}

/** Exported for the regression test that pins the OS actions to the host
 * predicate rather than to an `invoke` — see `ArtifactsView.osactions.test.tsx`. */
export function ArtifactDetail({
  artifact,
  folderOptions,
  onBack,
  onPatch,
  onDeleted,
  onReloaded,
  onError,
}: {
  artifact: Artifact;
  folderOptions: string[];
  onBack: () => void;
  onPatch: (
    id: string,
    fields: { title?: string; note?: string; starred?: boolean; path?: string },
  ) => void;
  onDeleted: () => void;
  onReloaded: () => Promise<void> | void;
  onError: (msg: string | null) => void;
}) {
  const { t } = useTranslation();
  const [note, setNote] = useState(artifact.note);
  const [exporting, setExporting] = useState(false);
  // Where this artifact currently shows up, editable. A text field with a
  // datalist rather than a picker: one control both files into an existing
  // folder and creates a new one by typing it, which is how a path field in a
  // file manager already behaves.
  const [folder, setFolder] = useState(artifact.path);
  /**
   * Which version the stage is previewing; null means the current one.
   *
   * Reset whenever the artifact changes — and whenever its current version
   * does, so that a rollback lands you on what is now current rather than
   * leaving you pinned to a version that just stopped being history.
   */
  const [previewVersion, setPreviewVersion] = useState<string | null>(null);

  useEffect(() => setNote(artifact.note), [artifact.id, artifact.note]);
  useEffect(() => setFolder(artifact.path), [artifact.id, artifact.path]);
  useEffect(() => setPreviewVersion(null), [artifact.id, artifact.currentVersion]);

  // Whether the two OS-level actions can do anything here. Deliberately a
  // synchronous host predicate and NOT the answer of an `invoke`: they used to
  // hang off an `artifact_local_path` call whose failure branch was a bare
  // `.catch(() => setLocalPath(null))`, so any hiccup on that one call — a
  // reject, or a command that simply took its time while the desktop's IPC was
  // busy (the debug log has `get_messages_tail` stalls up to 5.2s) — silently
  // erased both buttons, with nothing on screen to say why and no retry. The
  // path they need is resolved host-side from the artifact id anyway, so the
  // frontend never needed it; the only real fork is the browser build, which
  // has no file manager to hand anything to, and that is what this answers.
  const osActions = canRevealPath();

  const doExport = async () => {
    setExporting(true);
    try {
      // A tab cannot be given a destination path — `save()` answers null there
      // and the button would silently do nothing. Hand the browser a download
      // instead, the way the wiki's export does.
      if (isWebBuild()) {
        await downloadArtifact(artifact.id, artifact.name);
        onError(null);
        return;
      }
      const dest = await save({ defaultPath: artifact.name });
      if (!dest) return;
      await invoke("export_artifact", { id: artifact.id, dest });
      onError(null);
    } catch (e) {
      onError(t("artifacts.export_failed", "导出失败：{{error}}", { error: String(e) }));
    } finally {
      setExporting(false);
    }
  };

  return (
    <div className={styles.detail}>
      <div className={styles.detail_bar}>
        <button className={styles.action} onClick={onBack}>
          ← {t("artifacts.panel_title", "产出")}
        </button>
        <span className={styles.detail_title} title={artifact.name}>
          {artifact.title}
        </span>
        <div className={styles.detail_actions}>
          {/* Disabled + relabelled while outstanding: the whole span is a
              native save panel plus a chunked copy (137 MB of zip is a real
              wait), and with no state at all a click that had not opened its
              panel yet was indistinguishable from a dead button. */}
          <button className={styles.action} onClick={doExport} disabled={exporting}>
            {exporting
              ? t("artifacts.exporting", "导出中…")
              : t("artifacts.export_short", "导出")}
          </button>
          {osActions && (
            <>
              <button
                className={styles.action}
                onClick={async () => {
                  // Not the opener plugin's `openPath`: that command is
                  // scope-checked and `opener:default` does not grant
                  // `allow-open-path`, so it rejected and — unawaited — did
                  // nothing at all. The backend command resolves the path host-
                  // side and opens it from Rust, which is not scope-checked.
                  try {
                    await invoke("open_artifact_external", { id: artifact.id });
                    onError(null);
                  } catch (e) {
                    onError(t("artifacts.open_failed", "打开失败：{{error}}", { error: String(e) }));
                  }
                }}
              >
                {t("artifacts.open_with", "用系统应用打开")}
              </button>
              <button
                className={styles.action}
                onClick={async () => {
                  // The blob can be gone by now — the drift banner below exists
                  // precisely because the source file moves under us. Without
                  // this the click is indistinguishable from a no-op.
                  //
                  // Resolved from the id host-side (like the button above)
                  // rather than by shipping a path to the frontend first: that
                  // extra round trip is exactly what used to decide whether
                  // this button existed at all.
                  try {
                    await invoke("reveal_artifact", { id: artifact.id });
                    onError(null);
                  } catch (e) {
                    onError(t("artifacts.reveal_failed", "显示失败：{{error}}", { error: String(e) }));
                  }
                }}
              >
                {t("artifacts.reveal", "在访达中显示")}
              </button>
            </>
          )}
          <button
            className={`${styles.action} ${styles.action_danger}`}
            onClick={async () => {
              if (
                !window.confirm(
                  t("artifacts.delete_confirm", "删除「{{title}}」？", {
                    title: artifact.title,
                  }),
                )
              ) {
                return;
              }
              try {
                await invoke("delete_artifact", { id: artifact.id });
                onDeleted();
              } catch (e) {
                onError(String(e));
              }
            }}
          >
            {t("artifacts.delete", "删除产出")}
          </button>
        </div>
      </div>

      <ArtifactStage artifact={artifact} version={previewVersion ?? undefined} />

      <div className={styles.detail_meta}>
        {artifact.drifted && (
          <div className={styles.drift_banner}>
            <TriangleAlert size={13} />
            <span>
              <b>{t("artifacts.drifted", "源文件已被改写")}</b> —{" "}
              {t("artifacts.drifted_hint", "入库时是硬链接，之后源文件被就地重写。")}
            </span>
          </div>
        )}
        <div className={styles.meta_row}>
          <span>{formatBytes(artifact.sizeBytes)}</span>
          <span>{artifact.mime}</span>
          <span title={artifact.workspacePath}>{artifact.workspaceName}</span>
          <span>{new Date(artifact.createdMs).toLocaleString()}</span>
        </div>
        <label className={styles.folder_row}>
          <Folder size={13} strokeWidth={1.5} />
          <span className={styles.folder_label}>{t("artifacts.folder", "所在文件夹")}</span>
          <input
            className={styles.folder_input}
            list={`artifact-folders-${artifact.id}`}
            value={folder}
            placeholder={t("artifacts.folder_placeholder", "工作区根目录")}
            onChange={(e) => setFolder(e.target.value)}
            // Same commit-on-blur rule as the note: one disk write per edit,
            // not one per keystroke.
            onBlur={() => {
              if (folder !== artifact.path) onPatch(artifact.id, { path: folder });
            }}
            onKeyDown={(e) => {
              if (e.key === "Enter") e.currentTarget.blur();
              if (e.key === "Escape") setFolder(artifact.path);
            }}
          />
          <datalist id={`artifact-folders-${artifact.id}`}>
            {folderOptions.map((option) => (
              <option key={option} value={option} />
            ))}
          </datalist>
        </label>
        <textarea
          className={styles.note_input}
          value={note}
          placeholder={t("artifacts.note_placeholder", "这份产出是什么、给谁的…")}
          onChange={(e) => setNote(e.target.value)}
          // Commit on blur, not on every keystroke: each save is a disk write
          // (and an HTTP round trip on a remote workspace).
          onBlur={() => {
            if (note !== artifact.note) onPatch(artifact.id, { note });
          }}
        />
        {artifact.versions.length > 1 && (
          <ArtifactVersions
            artifact={artifact}
            previewVersion={previewVersion}
            onPreview={setPreviewVersion}
            onReloaded={onReloaded}
            onError={onError}
          />
        )}
      </div>
    </div>
  );
}

/**
 * Anything the stage can render: an artifact, or one member of a zip.
 *
 * The stage used to take an `Artifact` and reach for `artifactBlobUrl` itself.
 * It takes this instead so a zip member — which has no id, and whose bytes
 * live behind a `blob:` URL — goes through the *same* dispatch. A `report.md`
 * must look identical whether it arrived loose or inside an archive, and one
 * renderer is the only way to keep that true.
 */
export interface StageItem {
  url: string;
  mime: string;
  /** The store's coarse bucket (`artifacts::kind_for`). */
  kind: string;
  title: string;
}

/**
 * An artifact's history.
 *
 * Only rendered when there is more than one version — a card with a single
 * ingest has no history worth a section, and showing "v1" alone would imply
 * the feature is doing something it is not.
 *
 * Selecting a row previews *that* version in the stage above without changing
 * what is stored; 恢复 is the separate, explicit act. That split matters:
 * looking at an old version is how you decide whether you want it back.
 */
function ArtifactVersions({
  artifact,
  previewVersion,
  onPreview,
  onReloaded,
  onError,
}: {
  artifact: Artifact;
  previewVersion: string | null;
  onPreview: (version: string | null) => void;
  onReloaded: () => Promise<void> | void;
  onError: (msg: string | null) => void;
}) {
  const { t } = useTranslation();
  const [busy, setBusy] = useState(false);
  const shown = previewVersion ?? artifact.currentVersion;

  const rollback = async (version: string) => {
    setBusy(true);
    try {
      await invoke("rollback_artifact", { id: artifact.id, version });
      onError(null);
      await onReloaded();
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.versions}>
      <div className={styles.versions_head}>
        <History size={13} strokeWidth={1.5} />
        <span>
          {t("artifacts.versions", "版本历史（{{count}} 个）", {
            count: artifact.versions.length,
          })}
        </span>
      </div>
      {artifact.versions.map((v) => {
        const isCurrent = v.id === artifact.currentVersion;
        return (
          <div
            key={v.id}
            className={`${styles.version_row} ${shown === v.id ? styles.version_row_on : ""}`}
          >
            <button
              type="button"
              className={styles.version_target}
              onClick={() => onPreview(isCurrent ? null : v.id)}
              title={v.sourcePath}
            >
              <span className={styles.version_id}>{v.id}</span>
              <span className={styles.version_time}>
                {new Date(v.addedMs).toLocaleString()}
              </span>
              <span className={styles.version_size}>{formatBytes(v.sizeBytes)}</span>
              {isCurrent && (
                <span className={styles.version_current}>
                  {t("artifacts.version_current", "当前")}
                </span>
              )}
            </button>
            {!isCurrent && (
              <button
                type="button"
                className={styles.version_restore}
                disabled={busy}
                onClick={() => rollback(v.id)}
              >
                {t("artifacts.version_restore", "恢复")}
              </button>
            )}
          </div>
        );
      })}
      {previewVersion && (
        <div className={styles.version_hint}>
          {t(
            "artifacts.version_previewing",
            "正在预览 {{version}}，存储的仍是 {{current}}。",
            { version: previewVersion, current: artifact.currentVersion },
          )}
        </div>
      )}
    </div>
  );
}

/**
 * The preview surface. Which element renders is decided by `kind`, plus one
 * sub-split inside the `text` bucket (see `textPreviewMode`) so a markdown or
 * html deliverable is shown rendered instead of as source. Both come off the
 * mime the store already derived, so no component sniffs extensions itself.
 *
 * `<video>` and the PDF `<iframe>` both point at `fleet-artifact://`, which is
 * the protocol that honours `Range`; that is what makes seeking work rather
 * than re-downloading. The webview has no Office viewer of its own — an
 * `<iframe>` at a .docx renders a blank frame — so the OOXML three get one in
 * JavaScript, lazily (see `OfficePreview`). Everything left over (legacy .doc /
 * .xls / .ppt, ODF, non-zip archives) still gets the typed placeholder with
 * 导出 / 打开 one click away in the bar above.
 */
function PreviewStage({ item }: { item: StageItem }) {
  const { t } = useTranslation();
  const [text, setText] = useState<string | null>(null);
  const url = item.url;
  // html goes to the frame by URL, so only the two rendered-from-source modes
  // pull the bytes into React.
  const textMode = item.kind === "text" ? textPreviewMode(item.mime) : null;
  const needsBody = textMode === "markdown" || textMode === "plain";

  useEffect(() => {
    if (!needsBody) {
      setText(null);
      return;
    }
    let alive = true;
    fetch(url)
      .then((r) => r.text())
      .then((body) => {
        if (alive) setText(body);
      })
      .catch(() => {
        if (alive) setText(null);
      });
    return () => {
      alive = false;
    };
  }, [needsBody, url]);

  if (item.kind === "image") {
    return (
      <div className={styles.stage}>
        <img src={url} alt={item.title} />
      </div>
    );
  }
  if (item.kind === "video") {
    return (
      <div className={styles.stage}>
        <video src={url} controls preload="metadata" />
      </div>
    );
  }
  if (item.kind === "audio") {
    return (
      <div className={styles.stage}>
        <audio src={url} controls />
      </div>
    );
  }
  if (item.kind === "pdf") {
    return (
      <div className={styles.stage}>
        <iframe className={styles.doc_frame} src={url} title={item.title} />
      </div>
    );
  }
  if (textMode === "html") {
    // allow-scripts but NOT allow-same-origin: an agent-produced page may run
    // its own JS while staying a cross-origin document with no reach into
    // Tauri IPC. Same policy as the wiki's html frame.
    return (
      <div className={styles.stage}>
        <iframe
          className={styles.doc_frame}
          sandbox="allow-scripts"
          src={url}
          title={item.title}
        />
      </div>
    );
  }
  if (textMode === "markdown") {
    return (
      <div className={styles.stage}>
        <div className={styles.markdown_body}>
          {text === null ? t("artifacts.loading", "加载中…") : <TextBlock text={text} />}
        </div>
      </div>
    );
  }
  if (textMode === "plain") {
    return (
      <div className={styles.stage}>
        <pre className={styles.text_pre}>{text ?? t("artifacts.loading", "加载中…")}</pre>
      </div>
    );
  }
  const office = officeMode(item.mime);
  if (office) {
    return (
      <div className={`${styles.stage} ${styles.stage_office}`}>
        <Suspense fallback={<div className={styles.no_preview_hint}>{t("artifacts.loading", "加载中…")}</div>}>
          <OfficePreview mode={office} url={url} title={item.title} />
        </Suspense>
      </div>
    );
  }
  const Icon = KIND_ICON[item.kind] ?? FileText;
  return (
    <div className={styles.stage}>
      <div className={styles.no_preview}>
        <Icon size={40} strokeWidth={1.1} />
        <div className={styles.no_preview_title}>
          {t("artifacts.no_preview_title", "这个格式没法在这里预览")}
        </div>
        <div className={styles.no_preview_hint}>
          {/* docx/xlsx/pptx render above and a .zip is browsable; what lands
              here is the legacy binary Office formats, ODF, tar/gz/7z and
              unknown blobs. */}
          {t("artifacts.no_preview_hint", "这个格式只能导出，或者用系统应用打开。")}
        </div>
      </div>
    </div>
  );
}

/**
 * Byte length of the version being previewed.
 *
 * The zip browser is told the archive's size up front so it can read the
 * central directory at the tail without a probe request — and when an older
 * version is pinned in the stage, `artifact.sizeBytes` describes the *current*
 * one. Reading a stale length would put the tail scan in the wrong place.
 */
function versionSize(artifact: Artifact, version?: string): number {
  if (!version) return artifact.sizeBytes;
  return artifact.versions.find((v) => v.id === version)?.sizeBytes ?? artifact.sizeBytes;
}

/**
 * Save one zip member to disk.
 *
 * A member's bytes live only in the webview — the store knows nothing about
 * what is inside an artifact — so this cannot go through `export_artifact`,
 * which streams by id. In a tab there is no save dialog to ask (`save()`
 * answers null there and the button would silently do nothing, the same trap
 * `doExport` documents), so the browser gets a download instead.
 */
async function exportMemberBytes(name: string, bytes: Uint8Array) {
  if (isWebBuild()) {
    const href = URL.createObjectURL(new Blob([bytes as BlobPart]));
    const a = document.createElement("a");
    a.href = href;
    a.download = name;
    a.click();
    URL.revokeObjectURL(href);
    return;
  }
  const dest = await save({ defaultPath: name });
  if (!dest) return;
  await invoke("export_bytes", { dest, bytes: Array.from(bytes) });
}

/**
 * What the detail pane shows for one artifact.
 *
 * A .zip gets a folder browser instead of a preview — it is the one archive
 * format with a directory at the tail, so listing it costs a couple of KB
 * rather than a download (see `shared-ts/zipDir.ts`). Its members render
 * through the very same `PreviewStage`, handed down as `renderPreview`, so a
 * member never grows a second, drifting renderer.
 */
export function ArtifactStage({
  artifact,
  version,
}: {
  artifact: Artifact;
  /** Preview this version instead of the current one. */
  version?: string;
}) {
  const url = artifactBlobUrl(artifact.id, artifact.name, version);
  if (artifact.kind === "archive" && isBrowsableArchive(artifact.mime)) {
    return (
      <div className={`${styles.stage} ${styles.stage_office}`}>
        <ZipBrowser
          url={url}
          size={versionSize(artifact, version)}
          renderPreview={(member) => <PreviewStage item={member} />}
          onExportMember={exportMemberBytes}
        />
      </div>
    );
  }
  return (
    <PreviewStage
      item={{ url, mime: artifact.mime, kind: artifact.kind, title: artifact.title }}
    />
  );
}
