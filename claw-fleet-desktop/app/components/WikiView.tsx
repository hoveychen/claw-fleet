import { invoke } from "@tauri-apps/api/core";
import {
  useCallback,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
  type SetStateAction,
} from "react";
import { useTranslation } from "react-i18next";
import { save } from "@tauri-apps/plugin-dialog";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  BookOpen,
  Check,
  ChevronDown,
  ChevronRight,
  ChevronLeft,
  Copy,
  Download,
  FileCode,
  FileStack,
  FileText,
  Folder,
  FolderInput,
  FolderOutput,
  Library,
  RefreshCw,
  Trash2,
  type LucideIcon,
} from "lucide-react";
import { TextBlock } from "./blocks/TextBlock";
import type { WikiLinkContext } from "../markdown/wikiLinks";
import { EmptyState } from "./EmptyState";
import { ConfirmDialog } from "./ConfirmDialog";
import { PromptDialog } from "./PromptDialog";
import { ContextMenu, type ContextMenuAnchor, type ContextMenuItem } from "./ContextMenu";
import { PageShell } from "./PageShell";
import { dropTargetAt, usePointerDrag } from "../hooks/usePointerDrag";
import { useUIStore } from "../store";
import styles from "./WikiView.module.css";

// ── Types (mirror claw-fleet-core/src/wiki.rs, camelCase serde) ──────────────

interface WikiVersion {
  id: string;
  publishedMs: number;
  sizeBytes: number;
  fileCount: number;
  sourcePath: string;
}

export interface WikiDoc {
  slug: string;
  title: string;
  kind: "html" | "htmlDir" | "markdown";
  entry: string;
  workspacePath: string;
  workspaceName: string;
  createdMs: number;
  updatedMs: number;
  currentVersion: string;
  versions: WikiVersion[];
}

interface WikiSearchHit {
  slug: string;
  field: "meta" | "content";
  snippet: string;
}

// A file-type icon per kind reads more "file-like" than a text badge, while the
// kind colour keeps markdown / single-page HTML / bundled-HTML-dir distinct.
const KIND_CONFIG: Record<WikiDoc["kind"], { short: string; cssClass: string; Icon: LucideIcon }> = {
  html: { short: "HTML", cssClass: "kind_html", Icon: FileCode },
  htmlDir: { short: "HTML dir", cssClass: "kind_dir", Icon: FileStack },
  markdown: { short: "Markdown", cssClass: "kind_md", Icon: FileText },
};

// ── Helpers ──────────────────────────────────────────────────────────────────

/**
 * URL of one wiki file through the fleet-wiki custom protocol. Built by hand:
 * convertFileSrc() percent-encodes the whole path (`/` → `%2F`), which
 * collapses the URL to a single segment and breaks relative-asset resolution
 * inside HTML docs.
 */
function wikiFileUrl(slug: string, version: string, relpath: string): string {
  const path = [slug, version, ...relpath.split("/")]
    .map(encodeURIComponent)
    .join("/");
  return navigator.userAgent.includes("Windows")
    ? `http://fleet-wiki.localhost/${path}`
    : `fleet-wiki://localhost/${path}`;
}

function relativeTime(ms: number): string {
  if (!ms) return "";
  const sec = Math.floor((Date.now() - ms) / 1000);
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

/** Render `text` with case-insensitive occurrences of `term` wrapped in <mark>. */
function highlightTerm(text: string, term: string): ReactNode {
  const t = term.trim();
  if (!t) return text;
  const lower = text.toLowerCase();
  const needle = t.toLowerCase();
  const parts: ReactNode[] = [];
  let i = 0;
  for (;;) {
    const at = lower.indexOf(needle, i);
    if (at < 0) break;
    if (at > i) parts.push(text.slice(i, at));
    parts.push(<mark key={at}>{text.slice(at, at + t.length)}</mark>);
    i = at + t.length;
  }
  if (i < text.length) parts.push(text.slice(i));
  return parts.length > 0 ? parts : text;
}

function formatBytes(bytes: number): string {
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)}MB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)}K`;
  return `${bytes}B`;
}

// ── Virtual directory tree ───────────────────────────────────────────────────

/** Everything after the last `/` — mirrors core's `wiki::slug_basename`. */
function slugBasename(slug: string): string {
  const at = slug.lastIndexOf("/");
  return at < 0 ? slug : slug.slice(at + 1);
}

/** Everything before the last `/`; "" when the slug sits at the tree root. */
function slugDirname(slug: string): string {
  const at = slug.lastIndexOf("/");
  return at < 0 ? "" : slug.slice(0, at);
}

// ── Doc actions (shared by the sidebar context menu and the detail header) ────

/**
 * Copies the `[[slug]]` reference rather than an on-disk path: the slug is the
 * doc's stable address, and it's what `fleet wiki cat` and the session
 * composer's @-mention both resolve. Returns false when the write was refused.
 */
async function copyDocRef(slug: string): Promise<boolean> {
  try {
    await writeText(`[[${slug}]]`);
    return true;
  } catch (e) {
    console.error("clipboard write failed:", e);
    return false;
  }
}

/** Prompts for a destination, then writes `version` of `doc` to it. */
async function exportDoc(doc: WikiDoc, version: string): Promise<void> {
  // Mirrors core's wiki::export_filename — kind decides the artifact shape,
  // and only the slug's last segment is a name (the rest are directories).
  const ext = doc.kind === "markdown" ? "md" : doc.kind === "html" ? "html" : "zip";
  const dest = await save({
    defaultPath: `${slugBasename(doc.slug)}.${ext}`,
    filters: [{ name: doc.title, extensions: [ext] }],
  });
  if (!dest) return;
  await invoke("export_wiki_doc", { slug: doc.slug, version, dest });
}

type DocNode = { type: "doc"; doc: WikiDoc };
type FolderNode = {
  type: "folder";
  /** Full prefix, e.g. "arch/deep" — unique, so it keys collapse state. */
  path: string;
  /** Last segment only, e.g. "deep". */
  name: string;
  children: TreeNode[];
  /** Docs at or below this folder. */
  docCount: number;
};
type TreeNode = DocNode | FolderNode;

/**
 * Fold a flat doc list into the virtual directory tree its slugs describe:
 * `arch/deep/storage` nests under folder `arch` → folder `arch/deep`. Nothing
 * on disk is nested — the tree exists only here and in the slug string.
 *
 * `docs` arrives newest-first, so folders and docs both keep recency order;
 * folders sort ahead of docs at each level.
 */
function buildTree(docs: WikiDoc[]): TreeNode[] {
  const root: FolderNode = { type: "folder", path: "", name: "", children: [], docCount: 0 };
  const folders = new Map<string, FolderNode>();

  for (const doc of docs) {
    const segments = doc.slug.split("/");
    let parent = root;
    for (let i = 0; i < segments.length - 1; i++) {
      const path = segments.slice(0, i + 1).join("/");
      let folder = folders.get(path);
      if (!folder) {
        folder = { type: "folder", path, name: segments[i], children: [], docCount: 0 };
        folders.set(path, folder);
        parent.children.push(folder);
      }
      folder.docCount++;
      parent = folder;
    }
    parent.children.push({ type: "doc", doc });
  }

  const sortLevel = (nodes: TreeNode[]): TreeNode[] => {
    const dirs = nodes.filter((n): n is FolderNode => n.type === "folder");
    for (const d of dirs) d.children = sortLevel(d.children);
    return [...dirs, ...nodes.filter((n) => n.type === "doc")];
  };
  return sortLevel(root.children);
}

/** True when doc `slug` lives at or below folder `path`. */
function slugUnderFolder(slug: string, path: string): boolean {
  return slug.startsWith(`${path}/`);
}

// ── Sort ─────────────────────────────────────────────────────────────────────

type SortKey = "recent" | "title" | "size" | "workspace";

/** A doc's size = its current version's bytes (0 when the version is missing). */
function docSize(d: WikiDoc): number {
  return d.versions.find((v) => v.id === d.currentVersion)?.sizeBytes ?? 0;
}

/**
 * Sort a copy of `docs`. `recent` keeps the backend's newest-first order (docs
 * already arrive that way), so it's a plain clone.
 */
function sortDocs(docs: WikiDoc[], key: SortKey): WikiDoc[] {
  const out = [...docs];
  switch (key) {
    case "title":
      return out.sort((a, b) => a.title.localeCompare(b.title));
    case "size":
      return out.sort((a, b) => docSize(b) - docSize(a));
    case "workspace":
      return out.sort(
        (a, b) => a.workspaceName.localeCompare(b.workspaceName) || b.updatedMs - a.updatedMs,
      );
    default:
      return out;
  }
}

// ── Component ────────────────────────────────────────────────────────────────

export function WikiView() {
  const { t } = useTranslation();
  const [docs, setDocs] = useState<WikiDoc[]>([]);
  const [loaded, setLoaded] = useState(false);
  const {
    query,
    workspaceFilter,
    sortKey,
    selectedFolder,
    selectedSlug,
    collapsedFolders,
  } = useUIStore((s) => s.mainViewState.wiki);
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const setQuery = (value: string) => updateMainViewState("wiki", { query: value });
  const setWorkspaceFilter = (value: string) =>
    updateMainViewState("wiki", { workspaceFilter: value });
  const setSortKey = (value: SortKey) => updateMainViewState("wiki", { sortKey: value });
  const setSelectedFolder = useCallback((next: SetStateAction<string | null>) => {
    const current = useUIStore.getState().mainViewState.wiki.selectedFolder;
    updateMainViewState("wiki", {
      selectedFolder: typeof next === "function" ? next(current) : next,
    });
  }, [updateMainViewState]);
  const setSelectedSlug = useCallback((next: SetStateAction<string | null>) => {
    const current = useUIStore.getState().mainViewState.wiki.selectedSlug;
    updateMainViewState("wiki", {
      selectedSlug: typeof next === "function" ? next(current) : next,
    });
  }, [updateMainViewState]);
  // Which virtual folder the grid is scoped to. `null` = 全部 (every doc).
  // Folder navigation always returns from the document detail to the list;
  // otherwise the hidden grid updates while the old document stays onscreen.
  const selectFolder = (path: string | null) => {
    setSelectedFolder(path);
    setSelectedSlug(null);
  };
  const [confirmDelete, setConfirmDelete] = useState<
    | { kind: "doc"; slug: string }
    | { kind: "version"; slug: string; version: string }
    | { kind: "folder"; path: string; count: number }
    | null
  >(null);

  const load = useCallback(async () => {
    try {
      const data = await invoke<WikiDoc[]>("list_wiki_docs");
      setDocs(data);
    } catch {
      // keep whatever we had
    } finally {
      setLoaded(true);
    }
  }, []);

  useEffect(() => {
    load();
  }, [load]);

  // Distinct workspaces for the filter dropdown, stable order.
  const workspaces = useMemo(() => {
    const seen = new Map<string, string>();
    for (const d of docs) {
      if (!seen.has(d.workspacePath)) seen.set(d.workspacePath, d.workspaceName);
    }
    return [...seen.entries()].map(([path, name]) => ({ path, name }));
  }, [docs]);

  // Full-text hits from the backend, keyed by slug. `null` while idle or
  // before the debounced search for the current query lands.
  const [hits, setHits] = useState<Map<string, WikiSearchHit> | null>(null);
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setHits(null);
      return;
    }
    const timer = setTimeout(() => {
      invoke<WikiSearchHit[]>("search_wiki_docs", { query: q })
        .then((res) => setHits(new Map(res.map((h) => [h.slug, h]))))
        .catch(() => setHits(null));
    }, 250);
    return () => clearTimeout(timer);
  }, [query]);

  const searching = query.trim().length > 0;

  // Workspace-filtered set drives the folder rail — the tree stays stable while
  // searching (search narrows the grid, not the rail's folder list).
  const wsFiltered = useMemo(
    () => docs.filter((d) => workspaceFilter === "all" || d.workspacePath === workspaceFilter),
    [docs, workspaceFilter],
  );

  // Docs shown in the centre grid: workspace ∩ (search OR folder scope), sorted.
  const gridDocs = useMemo(() => {
    const q = query.trim().toLowerCase();
    let base = wsFiltered;
    if (q) {
      // Search flattens across folders — the rail selection is ignored so a hit
      // in another folder still shows.
      base = base.filter((d) =>
        hits
          ? hits.has(d.slug)
          : [d.title, d.slug, d.workspaceName].join(" ").toLowerCase().includes(q),
      );
    } else if (selectedFolder) {
      base = base.filter((d) => slugUnderFolder(d.slug, selectedFolder));
    }
    return sortDocs(base, sortKey);
  }, [wsFiltered, query, hits, selectedFolder, sortKey]);

  // Look up in the full doc set (not the grid) so cross-doc link jumps land even
  // when the target is hidden by the current search / workspace / folder scope.
  const selected = useMemo(
    () => docs.find((d) => d.slug === selectedSlug) ?? null,
    [docs, selectedSlug],
  );

  const wikiLinks = useMemo(() => {
    const slugs = new Set(docs.map((d) => d.slug));
    return {
      hasSlug: (slug: string) => slugs.has(slug),
      openSlug: setSelectedSlug,
    };
  }, [docs]);

  // Folder rail tree — folders only, off the workspace-filtered set.
  const tree = useMemo(() => buildTree(wsFiltered), [wsFiltered]);

  const collapsed = useMemo(() => new Set(collapsedFolders), [collapsedFolders]);
  const toggleFolder = (path: string) => {
    const next = new Set(collapsed);
    if (next.has(path)) next.delete(path);
    else next.add(path);
    updateMainViewState("wiki", { collapsedFolders: [...next] });
  };

  const handleDelete = async () => {
    if (!confirmDelete) return;
    try {
      if (confirmDelete.kind === "doc") {
        await invoke("delete_wiki_doc", { slug: confirmDelete.slug });
        if (selectedSlug === confirmDelete.slug) setSelectedSlug(null);
      } else if (confirmDelete.kind === "folder") {
        await invoke("delete_wiki_folder", { prefix: confirmDelete.path });
        if (selectedSlug?.startsWith(`${confirmDelete.path}/`)) setSelectedSlug(null);
        if (
          selectedFolder &&
          (selectedFolder === confirmDelete.path ||
            selectedFolder.startsWith(`${confirmDelete.path}/`))
        )
          setSelectedFolder(null);
      } else {
        await invoke("delete_wiki_version", {
          slug: confirmDelete.slug,
          version: confirmDelete.version,
        });
      }
    } catch (e) {
      console.error("wiki delete failed:", e);
    }
    setConfirmDelete(null);
    load();
  };

  // ── Moving a doc ───────────────────────────────────────────────────────────
  // Moving a doc IS re-keying it — the slug's directory prefix is the only
  // thing a folder ever was. Two entry points share `moveDoc`: dragging onto a
  // folder (prefix swap) and the context menu's dialog (arbitrary new slug,
  // which is how a folder gets created in the first place).
  const [dragSlug, setDragSlug] = useState<string | null>(null);
  const [dropPath, setDropPath] = useState<string | null>(null);
  const [moveError, setMoveError] = useState<string | null>(null);

  const moveDoc = useCallback(
    async (from: string, to: string) => {
      await invoke<WikiDoc>("move_wiki_doc", { from, to });
      setSelectedSlug((cur) => (cur === from ? to : cur));
      await load();
    },
    [load],
  );

  // Which card the current press started on. The drag hook is called once for
  // the whole view (rules of hooks), so the slug travels in a ref that each
  // card's pointerdown stamps.
  const pressedSlug = useRef<string | null>(null);
  const cardDrag = usePointerDrag({
    onStart: () => {
      const slug = pressedSlug.current;
      if (!slug) return false;
      setDragSlug(slug);
    },
    onMove: (p) => setDropPath(dropTargetAt(p.over, "data-drop-folder")),
    onDrop: (p) => {
      const path = dropTargetAt(p.over, "data-drop-folder");
      if (path === null) {
        setDragSlug(null);
        setDropPath(null);
        return;
      }
      handleDrop(path);
    },
    onCancel: () => {
      setDragSlug(null);
      setDropPath(null);
    },
  });

  const handleDrop = async (folderPath: string) => {
    const from = pressedSlug.current ?? dragSlug;
    pressedSlug.current = null;
    setDragSlug(null);
    setDropPath(null);
    if (!from) return;
    const to = folderPath ? `${folderPath}/${slugBasename(from)}` : slugBasename(from);
    if (to === from) return;
    setMoveError(null);
    try {
      await moveDoc(from, to);
    } catch (e) {
      setMoveError(String(e));
    }
  };

  // ── Context menu + move dialog ─────────────────────────────────────────────
  const [ctxMenu, setCtxMenu] = useState<{ doc: WikiDoc; anchor: ContextMenuAnchor } | null>(null);
  const [moveTarget, setMoveTarget] = useState<WikiDoc | null>(null);
  const [moveDialogError, setMoveDialogError] = useState<string | null>(null);

  const openMoveDialog = (doc: WikiDoc) => {
    setMoveDialogError(null);
    setMoveTarget(doc);
  };

  const handleMoveSubmit = async (to: string) => {
    if (!moveTarget) return;
    try {
      await moveDoc(moveTarget.slug, to);
      setMoveTarget(null);
    } catch (e) {
      // Keep the dialog open so the rejected slug stays editable.
      setMoveDialogError(String(e));
    }
  };

  // ── Folder ops ─────────────────────────────────────────────────────────────
  const [folderCtx, setFolderCtx] = useState<{ path: string; anchor: ContextMenuAnchor } | null>(
    null,
  );
  const [renameFolder, setRenameFolder] = useState<string | null>(null);
  const [folderError, setFolderError] = useState<string | null>(null);

  /**
   * Docs a folder op will actually touch. Counted over the full `docs` set, NOT
   * a filtered view: the backend acts on every doc under the prefix, so a count
   * taken from a filtered view would under-report what delete destroys.
   */
  const docsUnder = useCallback(
    (path: string) => docs.filter((d) => d.slug.startsWith(`${path}/`)).length,
    [docs],
  );

  /** Re-key a whole folder. `to` of "" dissolves it into the tree root. */
  const moveFolder = useCallback(
    async (from: string, to: string) => {
      await invoke<WikiDoc[]>("move_wiki_folder", { from, to });
      setSelectedSlug((cur) => {
        if (!cur?.startsWith(`${from}/`)) return cur;
        const rest = cur.slice(from.length + 1);
        return to ? `${to}/${rest}` : rest;
      });
      setSelectedFolder((cur) => {
        if (cur === null) return cur;
        if (cur === from) return to || null;
        if (cur.startsWith(`${from}/`)) {
          const rest = cur.slice(from.length + 1);
          return to ? `${to}/${rest}` : rest;
        }
        return cur;
      });
      await load();
    },
    [load],
  );

  const handleRenameFolderSubmit = async (to: string) => {
    if (renameFolder === null) return;
    try {
      await moveFolder(renameFolder, to);
      setRenameFolder(null);
    } catch (e) {
      setFolderError(String(e));
    }
  };

  const folderMenuItems = (path: string): ContextMenuItem[] => [
    {
      id: "rename",
      label: t("wiki.folder_rename", "重命名目录…"),
      icon: <FolderInput size={13} strokeWidth={1.7} />,
      onSelect: () => {
        setFolderError(null);
        setRenameFolder(path);
      },
    },
    {
      id: "dissolve",
      label: t("wiki.folder_dissolve", "解散目录（文档移到上级）"),
      icon: <FolderOutput size={13} strokeWidth={1.7} />,
      onSelect: () => {
        // Dissolving is just a move to the parent prefix — `p/a/x` → `p/x`.
        moveFolder(path, slugDirname(path)).catch((e) => setMoveError(String(e)));
      },
    },
    {
      id: "delete",
      label: t("wiki.folder_delete", "删除目录及其 {{count}} 篇文档", { count: docsUnder(path) }),
      icon: <Trash2 size={13} strokeWidth={1.7} />,
      danger: true,
      onSelect: () => setConfirmDelete({ kind: "folder", path, count: docsUnder(path) }),
    },
  ];

  const menuItems = (doc: WikiDoc): ContextMenuItem[] => [
    {
      id: "move",
      label: t("wiki.move", "移动 / 重命名…"),
      icon: <FolderInput size={13} strokeWidth={1.7} />,
      onSelect: () => openMoveDialog(doc),
    },
    {
      id: "copy",
      label: t("wiki.copy_ref_short", "复制引用"),
      icon: <Copy size={13} strokeWidth={1.7} />,
      onSelect: () => void copyDocRef(doc.slug),
    },
    {
      id: "export",
      label: t("wiki.export_short", "导出"),
      icon: <Download size={13} strokeWidth={1.7} />,
      onSelect: () => {
        exportDoc(doc, doc.currentVersion).catch((e) => console.error("wiki export failed:", e));
      },
    },
    {
      id: "delete",
      label: t("wiki.delete_doc", "删除文档"),
      icon: <Trash2 size={13} strokeWidth={1.7} />,
      danger: true,
      onSelect: () => setConfirmDelete({ kind: "doc", slug: doc.slug }),
    },
  ];

  /**
   * Mark an element as folder `path`'s drop zone ("" is the root). The card drag
   * resolves its landing spot by hit-testing this attribute (`dropTargetAt`), so
   * a nested folder row naturally wins over the root zone it sits inside —
   * `closest` stops at the first match, no stopPropagation needed.
   */
  const dropProps = (path: string) => ({ "data-drop-folder": path });

  /** Indent one tree level; depth 0 sits flush with the rail. */
  const indent = (depth: number) => ({ paddingLeft: `${8 + depth * 12}px` });

  // ── Folder rail (folders only) ───────────────────────────────────────────
  const renderFolderNodes = (nodes: TreeNode[], depth: number): ReactNode =>
    nodes
      .filter((n): n is FolderNode => n.type === "folder")
      .map((folder) => {
        const hasSub = folder.children.some((c) => c.type === "folder");
        const isCollapsed = collapsed.has(folder.path);
        const active = selectedFolder === folder.path;
        return (
          <div key={`d:${folder.path}`} className={styles.folder_group}>
            <button
              className={`${styles.folder_row} ${active ? styles.folder_row_active : ""} ${
                dropPath === folder.path ? styles.drop_target : ""
              }`}
              style={indent(depth)}
              onClick={() => selectFolder(folder.path)}
              onContextMenu={(e) => {
                e.preventDefault();
                e.stopPropagation();
                setFolderCtx({ path: folder.path, anchor: { x: e.clientX, y: e.clientY } });
              }}
              title={folder.path}
              {...dropProps(folder.path)}
            >
              <span
                className={styles.folder_chevron}
                onClick={(e) => {
                  // Toggle collapse without changing the grid scope.
                  if (!hasSub) return;
                  e.stopPropagation();
                  toggleFolder(folder.path);
                }}
              >
                {hasSub ? (
                  isCollapsed ? (
                    <ChevronRight size={12} strokeWidth={2} />
                  ) : (
                    <ChevronDown size={12} strokeWidth={2} />
                  )
                ) : null}
              </span>
              <Folder size={13} strokeWidth={1.7} className={styles.folder_icon} />
              <span className={styles.folder_name}>{folder.name}</span>
              <span className={styles.folder_count}>{folder.docCount}</span>
            </button>
            {hasSub && !isCollapsed && renderFolderNodes(folder.children, depth + 1)}
          </div>
        );
      });

  // ── Centre grid card ──────────────────────────────────────────────────────
  const renderGridCard = (d: WikiDoc, extraClass = ""): ReactNode => (
    <button
      key={`g:${d.slug}`}
      className={`${styles.grid_card} ${extraClass} ${
        selectedSlug === d.slug ? styles.grid_card_active : ""
      } ${dragSlug === d.slug ? styles.card_dragging : ""}`}
      onClick={() => {
        // The click a finished drag leaves behind must not also select the card.
        if (cardDrag.didDrag()) return;
        setSelectedSlug(d.slug);
      }}
      onContextMenu={(e) => {
        // preventDefault also suppresses the app-wide Settings/About menu.
        e.preventDefault();
        e.stopPropagation();
        setCtxMenu({ doc: d, anchor: { x: e.clientX, y: e.clientY } });
      }}
      // Pointer drag, not HTML5 drag: the latter never delivers a drop inside
      // this app's webview. See usePointerDrag.ts.
      onPointerDown={(e) => {
        pressedSlug.current = d.slug;
        cardDrag.onPointerDown(e);
      }}
    >
      <span className={styles.grid_card_top}>
        {(() => {
          const cfg = KIND_CONFIG[d.kind];
          const Icon = cfg?.Icon ?? FileText;
          return (
            <span
              className={`${styles.kind_icon} ${styles[cfg?.cssClass ?? "kind_md"]}`}
              title={cfg?.short ?? d.kind}
            >
              <Icon size={16} strokeWidth={1.7} />
            </span>
          );
        })()}
        {d.versions.length > 1 && (
          <span className={styles.grid_versions}>
            {t("wiki.version_count", "{{count}} versions", { count: d.versions.length })}
          </span>
        )}
      </span>
      <span className={styles.grid_title}>
        {searching ? highlightTerm(d.title, query) : d.title}
      </span>
      <span className={styles.grid_slug} title={d.slug}>
        {d.slug}
      </span>
      {searching && hits?.get(d.slug)?.snippet ? (
        <span className={styles.grid_snippet}>
          {highlightTerm(hits.get(d.slug)!.snippet, query)}
        </span>
      ) : null}
      <span className={styles.grid_meta}>
        {workspaceFilter === "all" && (
          <span className={styles.ws_badge} title={d.workspacePath}>
            {d.workspaceName}
          </span>
        )}
        <span className={styles.grid_time}>{relativeTime(d.updatedMs)}</span>
      </span>
    </button>
  );

  // ── Folder overview tile (landing page) ────────────────────────────────────
  const renderFolderTile = (folder: FolderNode): ReactNode => (
    <button
      key={`ft:${folder.path}`}
      className={styles.folder_tile}
      onClick={() => selectFolder(folder.path)}
      onContextMenu={(e) => {
        e.preventDefault();
        e.stopPropagation();
        setFolderCtx({ path: folder.path, anchor: { x: e.clientX, y: e.clientY } });
      }}
      title={folder.path}
      {...dropProps(folder.path)}
    >
      <Folder size={17} strokeWidth={1.6} className={styles.folder_tile_icon} />
      <span className={styles.folder_tile_name}>{folder.name}</span>
      <span className={styles.folder_tile_count}>
        {t("wiki.doc_count", "{{count}} docs", { count: folder.docCount })}
      </span>
    </button>
  );

  // The landing = the default resting state: 全部文档, no search, so the grid
  // leads with a folder overview before the flat doc list. Scoping to a folder
  // or searching drops straight to the plain doc grid.
  const showLanding = selectedFolder === null && !searching;
  const topFolders = tree.filter((n): n is FolderNode => n.type === "folder");
  // Recency-pinned quick strip for the landing — always newest-first regardless
  // of the grid's sort dropdown, so it stays a stable "jump to what changed".
  const recentDocs = [...wsFiltered].sort((a, b) => b.updatedMs - a.updatedMs).slice(0, 6);

  // Breadcrumb-ish label for the grid header.
  const scopeLabel = searching
    ? t("wiki.scope_search", "搜索结果")
    : selectedFolder ?? t("wiki.scope_all", "全部文档");

  return (
    <PageShell
      view="wiki"
      className={styles.wiki_scope}
      title={t("wiki.panel_title", "知识库")}
      count={loaded && docs.length > 0 ? docs.length : null}
      search={{
        value: query,
        onChange: setQuery,
        placeholder: t("wiki.search_placeholder", "Search docs…"),
      }}
      bannerCenter={
        <select
          className={styles.ws_select}
          value={workspaceFilter}
          onChange={(e) => setWorkspaceFilter(e.target.value)}
        >
          <option value="all">{t("wiki.all_workspaces", "All workspaces")}</option>
          {workspaces.map((ws) => (
            <option key={ws.path} value={ws.path}>
              {ws.name}
            </option>
          ))}
        </select>
      }
      actions={
        <button className={styles.icon_btn} onClick={load} title={t("wiki.refresh", "Refresh")}>
          <RefreshCw size={13} strokeWidth={1.7} />
        </button>
      }
      secondary={
        // The rail is the folder tree AND the root drop zone: dropping onto the
        // pane background (outside any folder) strips a doc's directory prefix.
        <div
          className={`${styles.rail_pane} ${dragSlug && dropPath === "" ? styles.drop_target_root : ""}`}
          {...dropProps("")}
        >
          <button
            className={`${styles.folder_row} ${styles.all_row} ${
              selectedFolder === null ? styles.folder_row_active : ""
            }`}
            onClick={() => selectFolder(null)}
          >
            <span className={styles.folder_chevron} />
            <Library size={13} strokeWidth={1.7} className={styles.folder_icon} />
            <span className={styles.folder_name}>{t("wiki.scope_all", "全部文档")}</span>
            <span className={styles.folder_count}>{wsFiltered.length}</span>
          </button>
          {loaded && wsFiltered.length > 0 && renderFolderNodes(tree, 0)}
        </div>
      }
      afterBody={<>
      {ctxMenu && (
        <ContextMenu
          anchor={ctxMenu.anchor}
          items={menuItems(ctxMenu.doc)}
          onClose={() => setCtxMenu(null)}
        />
      )}

      {folderCtx && (
        <ContextMenu
          anchor={folderCtx.anchor}
          items={folderMenuItems(folderCtx.path)}
          onClose={() => setFolderCtx(null)}
        />
      )}

      {renameFolder !== null && (
        <PromptDialog
          title={t("wiki.folder_rename_title", "重命名目录 “{{path}}”", { path: renameFolder })}
          hint={t(
            "wiki.folder_rename_hint",
            "目录下的 {{count}} 篇文档会一起改前缀。写成 a/b 可把它移到别的目录下；移入已存在的目录会合并，撞名则整体失败、什么都不改。",
            { count: docsUnder(renameFolder) },
          )}
          defaultValue={renameFolder}
          confirmLabel={t("wiki.folder_rename_confirm", "重命名")}
          error={folderError}
          onConfirm={handleRenameFolderSubmit}
          onCancel={() => setRenameFolder(null)}
        />
      )}

      {moveTarget && (
        <PromptDialog
          title={t("wiki.move_title", "移动 / 重命名 “{{title}}”", { title: moveTarget.title })}
          hint={t(
            "wiki.move_hint",
            "输入新的 slug。用 / 分隔目录 —— 写成 design/foo 就会建出 design 目录。最多 8 层，每段 ≤64 字符。",
          )}
          defaultValue={moveTarget.slug}
          confirmLabel={t("wiki.move_confirm", "移动")}
          error={moveDialogError}
          onConfirm={handleMoveSubmit}
          onCancel={() => setMoveTarget(null)}
        />
      )}

      {confirmDelete && (
        <ConfirmDialog
          message={
            confirmDelete.kind === "doc"
              ? t("wiki.delete_doc_confirm", "Delete “{{slug}}” and all its versions?", {
                  slug: confirmDelete.slug,
                })
              : confirmDelete.kind === "folder"
                ? t(
                    "wiki.delete_folder_confirm",
                    "删除目录 “{{path}}” 下的全部 {{count}} 篇文档（含所有历史版本）？此操作不可撤销。",
                    { path: confirmDelete.path, count: confirmDelete.count },
                  )
                : t("wiki.delete_version_confirm", "Delete version {{version}} of “{{slug}}”?", {
                    slug: confirmDelete.slug,
                    version: confirmDelete.version,
                  })
          }
          onConfirm={handleDelete}
          onCancel={() => setConfirmDelete(null)}
        />
      )}
      </>}
    >
      <div className={styles.layout}>
        <div className={`${styles.grid_pane} ${selected ? styles.grid_pane_split : ""}`}>
          <div className={styles.grid_header}>
            <span className={styles.grid_scope} title={scopeLabel}>
              {scopeLabel}
            </span>
            <span className={styles.grid_scope_count}>{gridDocs.length}</span>
            <span className={styles.grid_header_spacer} />
            <select
              className={styles.sort_select}
              value={sortKey}
              onChange={(e) => setSortKey(e.target.value as SortKey)}
              title={t("wiki.sort_by", "Sort by")}
            >
              <option value="recent">{t("wiki.sort_recent", "最近更新")}</option>
              <option value="title">{t("wiki.sort_title", "名称")}</option>
              <option value="size">{t("wiki.sort_size", "大小")}</option>
              <option value="workspace">{t("wiki.sort_workspace", "工作区")}</option>
            </select>
          </div>
          {moveError && (
            <p className={styles.move_error} onClick={() => setMoveError(null)}>
              {t("wiki.move_failed", "Move failed: {{error}}", { error: moveError })}
            </p>
          )}
          {!loaded && <p className={styles.empty}>{t("wiki.loading", "Loading…")}</p>}
          {loaded && gridDocs.length === 0 && (
            <EmptyState
              icon={<BookOpen size={28} strokeWidth={1.5} />}
              title={
                docs.length === 0
                  ? t("wiki.empty_title", "No wiki docs yet")
                  : t("wiki.empty_scope_title", "这个范围没有文档")
              }
              subtitle={
                docs.length === 0
                  ? t(
                      "wiki.empty_subtitle",
                      "Agents publish reports and demos here with `fleet wiki publish <path>`.",
                    )
                  : t("wiki.empty_scope_subtitle", "换个目录或清空搜索试试。")
              }
            />
          )}
          {loaded && gridDocs.length > 0 && (
            <div className={styles.grid_scroll}>
              {showLanding && (
                <>
                  {recentDocs.length > 0 && (
                    <>
                      <div className={styles.section_head}>{t("wiki.section_recent", "最近更新")}</div>
                      <div className={styles.recent_row}>
                        {recentDocs.map((d) => renderGridCard(d, styles.recent_card))}
                      </div>
                    </>
                  )}
                  {topFolders.length > 0 && (
                    <>
                      <div className={styles.section_head}>{t("wiki.section_folders", "目录")}</div>
                      <div className={styles.folder_tiles}>{topFolders.map(renderFolderTile)}</div>
                    </>
                  )}
                  <div className={styles.section_head}>{t("wiki.section_docs", "文档")}</div>
                </>
              )}
              <div className={styles.grid}>{gridDocs.map((d) => renderGridCard(d))}</div>
            </div>
          )}
        </div>

        {selected && (
          <div className={styles.preview_pane}>
            <WikiDetail
              doc={selected}
              wikiLinks={wikiLinks}
              onClose={() => setSelectedSlug(null)}
              onMove={() => openMoveDialog(selected)}
              onDeleteDoc={() => setConfirmDelete({ kind: "doc", slug: selected.slug })}
              onDeleteVersion={(version) =>
                setConfirmDelete({ kind: "version", slug: selected.slug, version })
              }
            />
          </div>
        )}
      </div>
    </PageShell>
  );
}

// ── Detail pane ──────────────────────────────────────────────────────────────

function WikiDetail({
  doc,
  wikiLinks,
  onClose,
  onMove,
  onDeleteDoc,
  onDeleteVersion,
}: {
  doc: WikiDoc;
  wikiLinks: WikiLinkContext;
  onClose: () => void;
  onMove: () => void;
  onDeleteDoc: () => void;
  onDeleteVersion: (version: string) => void;
}) {
  const { t } = useTranslation();
  const versionBySlug = useUIStore((s) => s.mainViewState.wiki.versionBySlug);
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const version = versionBySlug[doc.slug] ?? doc.currentVersion;
  const setVersion = (nextVersion: string) =>
    updateMainViewState("wiki", {
      versionBySlug: { ...versionBySlug, [doc.slug]: nextVersion },
    });
  const effectiveVersion = doc.versions.some((v) => v.id === version)
    ? version
    : doc.currentVersion;

  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const id = setTimeout(() => setCopied(false), 1500);
    return () => clearTimeout(id);
  }, [copied]);
  const handleCopyRef = async () => {
    if (await copyDocRef(doc.slug)) setCopied(true);
  };

  const [exporting, setExporting] = useState(false);
  const handleExport = async () => {
    setExporting(true);
    try {
      await exportDoc(doc, effectiveVersion);
    } catch (e) {
      console.error("wiki export failed:", e);
    } finally {
      setExporting(false);
    }
  };

  return (
    <>
      <div className={styles.detail_header}>
        <button
          className={styles.back_btn}
          onClick={onClose}
          title={t("wiki.back", "返回列表")}
        >
          <ChevronLeft size={14} strokeWidth={2} />
          {t("wiki.back_short", "返回")}
        </button>
        <div className={styles.detail_title}>
          {doc.workspaceName}
          <span className={styles.detail_sep}>/</span>
          <span className={styles.detail_name}>{doc.title}</span>
        </div>
        <div className={styles.detail_actions}>
          <select
            className={styles.version_select}
            value={effectiveVersion}
            onChange={(e) => setVersion(e.target.value)}
          >
            {doc.versions.map((v) => (
              <option key={v.id} value={v.id}>
                {v.id}
                {v.id === doc.currentVersion ? ` (${t("wiki.current", "current")})` : ""}
                {` · ${formatBytes(v.sizeBytes)}`}
              </option>
            ))}
          </select>
          <button
            className={styles.action_btn}
            onClick={onMove}
            title={t("wiki.move_title_short", "Move or rename — type a slug with / to make a folder")}
          >
            <FolderInput size={12} strokeWidth={1.7} />
            {t("wiki.move_short", "移动")}
          </button>
          <button
            className={styles.action_btn}
            onClick={handleCopyRef}
            title={t("wiki.copy_ref", "Copy [[slug]] — paste it into a session prompt")}
          >
            {copied ? <Check size={12} strokeWidth={1.7} /> : <Copy size={12} strokeWidth={1.7} />}
            {copied ? t("wiki.copied", "Copied") : t("wiki.copy_ref_short", "Copy ref")}
          </button>
          <button
            className={styles.action_btn}
            onClick={handleExport}
            disabled={exporting}
            title={t("wiki.export", "Export this version to a file")}
          >
            <Download size={12} strokeWidth={1.7} />
            {t("wiki.export_short", "Export")}
          </button>
          {effectiveVersion !== doc.currentVersion && (
            <button
              className={styles.action_btn}
              onClick={() => onDeleteVersion(effectiveVersion)}
              title={t("wiki.delete_version", "Delete this version")}
            >
              <Trash2 size={12} strokeWidth={1.7} />
              {t("wiki.delete_version_short", "Version")}
            </button>
          )}
          <button
            className={`${styles.action_btn} ${styles.action_danger}`}
            onClick={onDeleteDoc}
            title={t("wiki.delete_doc", "Delete doc")}
          >
            <Trash2 size={12} strokeWidth={1.7} />
            {t("wiki.delete_doc_short", "Doc")}
          </button>
        </div>
      </div>

      <div className={styles.detail_body_wrap}>
        <WikiDocBody doc={doc} version={effectiveVersion} wikiLinks={wikiLinks} />
      </div>
    </>
  );
}

/**
 * One wiki doc's content — markdown fetched and rendered, or the bundled HTML
 * in its own frame. Everything the reader looks at, none of the chrome.
 *
 * Split out of `WikiDetail` (which keeps the header full of doc actions) so a
 * detail-column wiki tab renders the identical body: same fetch, same link
 * context, same sandbox policy. `version` is already resolved by the caller,
 * which is what lets the tab share the wiki page's per-slug version choice.
 */
export function WikiDocBody({
  doc,
  version,
  wikiLinks,
}: {
  doc: WikiDoc;
  version: string;
  wikiLinks: WikiLinkContext;
}) {
  const { t } = useTranslation();
  const [markdown, setMarkdown] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (doc.kind !== "markdown") {
      setMarkdown(null);
      return;
    }
    setLoading(true);
    setError(null);
    let stale = false;
    invoke<string>("get_wiki_file_text", {
      slug: doc.slug,
      version,
      relpath: doc.entry,
    })
      .then((text) => {
        if (!stale) setMarkdown(text);
      })
      .catch((e) => {
        if (!stale) setError(String(e));
      })
      .finally(() => {
        if (!stale) setLoading(false);
      });
    return () => {
      stale = true;
    };
  }, [doc.slug, doc.kind, doc.entry, version]);

  if (doc.kind !== "markdown") {
    // allow-scripts but NOT allow-same-origin: demos can run JS while staying a
    // cross-origin document with no reach into Tauri IPC.
    return (
      <iframe
        key={`${doc.slug}/${version}`}
        className={styles.html_frame}
        sandbox="allow-scripts"
        src={wikiFileUrl(doc.slug, version, doc.entry)}
        title={doc.title}
      />
    );
  }

  return (
    <div className={styles.markdown_body}>
      {loading && <p className={styles.loading}>{t("wiki.loading", "Loading…")}</p>}
      {error && <p className={styles.error}>{error}</p>}
      {markdown !== null && (
        <div className={styles.content_markdown}>
          <TextBlock text={markdown} wiki={wikiLinks} />
        </div>
      )}
    </div>
  );
}
