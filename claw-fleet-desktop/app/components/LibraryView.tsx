import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Archive,
  BookOpen,
  ChevronDown,
  ChevronRight,
  Film,
  FileSpreadsheet,
  FileText,
  FileType,
  FileUp,
  Folder,
  FolderGit2,
  FolderOpen,
  FolderPlus,
  FolderUp,
  Image as ImageIcon,
  Inbox,
  Library,
  Music,
  NotebookPen,
  Pencil,
  Presentation,
  Trash2,
} from "lucide-react";

import { useUIStore } from "../store";
import { useWikiDocsStore } from "../hooks/useWikiDocs";
import { dropTargetAt, usePointerDrag } from "../hooks/usePointerDrag";
import {
  buildLibraryTree,
  libraryDropKey,
  libraryDropTarget,
  libraryEntryKey,
  libraryEntryMatches,
  type LibraryEntry,
  type LibraryFolderNode,
  type LibraryProjectTree,
} from "../../../shared-ts/libraryTree";
import {
  ingestBatchIdOf,
  ingestBatchKey,
  isIngestRunning,
  type IngestBatch,
  type IngestBatchSummary,
} from "../../../shared-ts/libraryIngest";
import type { AuxDoc } from "../detailAux";
import {
  filesFromDataTransfer,
  pickBrowserFiles,
  pickLocalPaths,
  startLocalUpload,
  uploadBrowserFiles,
  type PickKind,
  type WebUploadFile,
} from "../libraryUpload";
import type { RegisteredProject } from "../projectList";
import type { AuxCardTail } from "./auxDocMenu";
import { ArtifactTabPane } from "./ArtifactTabPane";
import { BATCH_POLL_MS, BatchDetail, BatchRow } from "./LibraryBatch";
import { WikiTabPane } from "./WikiTabPane";
import { EmptyState } from "./EmptyState";
import { PageShell } from "./PageShell";
import { SkeletonList, Spinner } from "./loading";
import { timeAgo } from "./timeAgo";
import styles from "./LibraryView.module.css";

const ARTIFACT_ICON: Record<string, typeof FileText> = {
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

/** Rows carrying this attribute are drop zones; the value is a
 *  `libraryDropKey(project, folder)` (`folder: ""` = the unfiled bucket). */
const DROP_ATTR = "data-library-drop";

/** A project branch carries its key here: files dropped from the OS onto it
 *  are uploaded into that project. */
const UPLOAD_ATTR = "data-library-upload";

/** A real Tauri webview (OS drops arrive as paths) rather than a browser tab
 *  (drops arrive as bytes). Same probe ChatComposer uses. */
function isTauriHost(): boolean {
  return typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
}

function uploadTargetAt(el: Element | null): string | null {
  return el?.closest(`[${UPLOAD_ATTR}]`)?.getAttribute(UPLOAD_ATTR) ?? null;
}

/** Wiki bodies matched by a full-text search, so a doc whose title does not
 *  contain the query still passes it. Debounced; a query shorter than two
 *  characters is answered from titles alone. */
function useWikiBodyHits(query: string): { slugs: ReadonlySet<string>; busy: boolean } {
  const [hits, setHits] = useState<{ q: string; slugs: Set<string> }>({ q: "", slugs: new Set() });
  const q = query.trim();
  useEffect(() => {
    if (q.length < 2) return;
    let alive = true;
    const timer = window.setTimeout(() => {
      invoke<Array<{ slug: string }>>("search_wiki_docs", { query: q })
        .then((rows) => {
          if (alive) setHits({ q, slugs: new Set((rows ?? []).map((r) => r.slug)) });
        })
        .catch(() => {
          if (alive) setHits({ q, slugs: new Set() });
        });
    }, 200);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [q]);
  const current = q.length >= 2 && hits.q === q;
  return { slugs: current ? hits.slugs : EMPTY, busy: q.length >= 2 && !current };
}

const EMPTY: ReadonlySet<string> = new Set();

/** The row a folder verb is running on, so it can show a spinner. */
type Busy = string | null;

/** Everything a folder or entry row needs from the page, passed down once. */
interface TreeCtx {
  project: string;
  selectedKey: string | null;
  dropActive: string | null;
  busy: Busy;
  draggingKey: string | null;
  searching: boolean;
  onOpen: (e: LibraryEntry) => void;
  entryDrag: (e: LibraryEntry) => { onPointerDown: (ev: React.PointerEvent) => void };
  onCreateFolder: (project: string, path: string) => void;
  onRenameFolder: (project: string, from: string, to: string) => void;
  onDeleteFolder: (project: string, path: string, label: string) => void;
  onDescribeFolder: (project: string, path: string, description: string) => void;
  onOpenBatch: (batchId: string) => void;
}

/**
 * 资料 (Library): wiki documents and artifacts as one folder tree per project.
 *
 * The folders are the project's schema (`claw_fleet_core::library`), each with
 * a one-line description; core places every doc and artifact in the folder it
 * is filed in, or in 未归档 when it is not filed in any schema folder. Both
 * stores stay as they are — moving an item re-keys a doc's slug or sets an
 * artifact's `path`, done server-side. The other two segments of this section
 * (文档 / 产出) are still each store's own management page. The detail column
 * reuses the reader panes a conversation's side rail opens.
 */
export function LibraryView() {
  const { t } = useTranslation();
  const { query, project, selectedKey } = useUIStore((s) => s.mainViewState.library);
  const update = useUIStore((s) => s.updateMainViewState);
  const set = useCallback(
    (patch: Partial<{ query: string; project: string; selectedKey: string | null }>) =>
      update("library", patch),
    [update],
  );

  const fetchDocs = useWikiDocsStore((s) => s.fetch);
  const [projects, setProjects] = useState<LibraryProjectTree[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<Busy>(null);

  const load = useCallback(async () => {
    try {
      setProjects((await invoke<LibraryProjectTree[]>("library_tree", { workspace: null })) ?? []);
    } catch (e) {
      setProjects((prev) => prev ?? []);
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // Registered projects that have no library yet: listed in the project
  // picker so a first upload has somewhere explicit to go.
  const [registered, setRegistered] = useState<RegisteredProject[]>([]);
  useEffect(() => {
    invoke<RegisteredProject[]>("list_projects")
      .then((r) => setRegistered((r ?? []).filter((p) => !p.archivedAtMs)))
      .catch(() => {});
  }, []);
  const allProjects = useMemo<LibraryProjectTree[]>(() => {
    const tree = projects ?? [];
    const extra = registered
      .filter((r) => !tree.some((p) => p.workspacePath === r.path))
      .map((r) => ({ workspacePath: r.path, workspaceName: r.name, folders: [], items: [] }))
      .sort((a, b) => a.workspaceName.localeCompare(b.workspaceName));
    return [...tree, ...extra];
  }, [projects, registered]);

  // ── Upload batches ────────────────────────────────────────────────────────
  const [batches, setBatches] = useState<IngestBatchSummary[]>([]);
  const [webUpload, setWebUpload] = useState<{ project: string; sent: number; total: number } | null>(null);
  const batchSig = useRef<string | null>(null);
  const loadBatches = useCallback(async () => {
    try {
      const list = (await invoke<IngestBatchSummary[]>("library_ingest_batches", { workspace: null })) ?? [];
      setBatches(list);
      // A batch that filed, finished or was undone changed the tree.
      const sig = list.map((b) => `${b.id}:${b.status}:${b.filed}:${b.undone}`).join("|");
      if (batchSig.current !== null && sig !== batchSig.current) {
        void load();
        void fetchDocs();
      }
      batchSig.current = sig;
    } catch {
      // The batch list is a companion to the tree; the tree still works.
    }
  }, [load, fetchDocs]);
  useEffect(() => {
    void loadBatches();
  }, [loadBatches]);
  const anyRunning = batches.some((b) => isIngestRunning(b.status));
  useEffect(() => {
    if (!anyRunning) return;
    const timer = window.setInterval(() => void loadBatches(), BATCH_POLL_MS);
    return () => window.clearInterval(timer);
  }, [anyRunning, loadBatches]);

  const afterStart = useCallback(
    (b: IngestBatch) => {
      setError(null);
      set({ selectedKey: ingestBatchKey(b.id) });
      void loadBatches();
    },
    [set, loadBatches],
  );
  const uploadPaths = useCallback(
    async (p: string, paths: string[]) => {
      if (paths.length === 0) return;
      try {
        afterStart(await startLocalUpload(p, paths));
      } catch (e) {
        setError(String(e));
      }
    },
    [afterStart],
  );
  const uploadWebFiles = useCallback(
    async (p: string, files: WebUploadFile[]) => {
      if (files.length === 0) return;
      setWebUpload({ project: p, sent: 0, total: files.length });
      try {
        afterStart(
          await uploadBrowserFiles(p, files, (sent, total) => setWebUpload({ project: p, sent, total })),
        );
      } catch (e) {
        setError(String(e));
      } finally {
        setWebUpload(null);
      }
    },
    [afterStart],
  );
  const onUpload = useCallback(
    (p: string, kind: PickKind) => {
      if (isTauriHost()) {
        pickLocalPaths(kind)
          .then((paths) => uploadPaths(p, paths))
          .catch((e) => setError(String(e)));
      } else {
        // Synchronous from the click: the browser only opens a file dialog
        // inside the user's activation window.
        void pickBrowserFiles(kind).then((files) => uploadWebFiles(p, files));
      }
    },
    [uploadPaths, uploadWebFiles],
  );

  // A stored project key from before the tree (a string-derived repo root) may
  // name nothing core knows; fall back to every project rather than a blank.
  const activeProject = useMemo(
    () => allProjects.find((p) => p.workspacePath === project)?.workspacePath ?? "",
    [allProjects, project],
  );

  // ── Dropping files from the OS ────────────────────────────────────────────
  // The target is the project branch under the pointer; with the page
  // filtered to one project (or only one existing) anywhere on the list will
  // do. Never a guess beyond that — the project must be explicit.
  const [dropProject, setDropProject] = useState<string | null>(null);
  const fallbackTarget = activeProject || (allProjects.length === 1 ? allProjects[0].workspacePath : "");
  const dropRefs = useRef({ fallbackTarget, uploadPaths, uploadWebFiles });
  dropRefs.current = { fallbackTarget, uploadPaths, uploadWebFiles };
  const noTargetMsg = t("library.drop_pick_project", "拖到某个项目上再松开（或先在上方选定项目）");
  const noTargetRef = useRef(noTargetMsg);
  noTargetRef.current = noTargetMsg;
  useEffect(() => {
    if (!isTauriHost()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    const targetAt = (pos: { x: number; y: number }) => {
      const dpr = window.devicePixelRatio > 0 ? window.devicePixelRatio : 1;
      return (
        uploadTargetAt(document.elementFromPoint(pos.x / dpr, pos.y / dpr)) ||
        dropRefs.current.fallbackTarget ||
        null
      );
    };
    (async () => {
      try {
        const { getCurrentWebview } = await import("@tauri-apps/api/webview");
        const un = await getCurrentWebview().onDragDropEvent((event) => {
          const p = event.payload;
          if (p.type === "enter" || p.type === "over") {
            setDropProject(targetAt(p.position));
          } else if (p.type === "leave") {
            setDropProject(null);
          } else if (p.type === "drop") {
            const target = targetAt(p.position);
            setDropProject(null);
            if (!p.paths?.length) return;
            if (!target) setError(noTargetRef.current);
            else void dropRefs.current.uploadPaths(target, p.paths);
          }
        });
        if (disposed) un();
        else unlisten = un;
      } catch {
        // Not a Tauri webview: the HTML5 handlers on the list take drops.
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  const onWebDragOver = (e: React.DragEvent) => {
    if (isTauriHost() || !e.dataTransfer.types.includes("Files")) return;
    e.preventDefault();
    setDropProject(uploadTargetAt(e.target as Element) || fallbackTarget || null);
  };
  const onWebDrop = (e: React.DragEvent) => {
    if (isTauriHost() || !e.dataTransfer.types.includes("Files")) return;
    e.preventDefault();
    const target = uploadTargetAt(e.target as Element) || fallbackTarget;
    setDropProject(null);
    if (!target) {
      setError(noTargetMsg);
      return;
    }
    // Entries are read synchronously inside; the walk itself is async.
    void filesFromDataTransfer(e.dataTransfer).then((files) => uploadWebFiles(target, files));
  };

  const bodyHits = useWikiBodyHits(query);
  const q = query.trim();
  const views = useMemo(() => {
    const keep = q
      ? (e: LibraryEntry) =>
          libraryEntryMatches(e, q) || (e.kind === "doc" && bodyHits.slugs.has(e.id))
      : undefined;
    // "All projects" lists those with a library; a picked project shows even
    // when it has none yet, so there is somewhere to upload to.
    return (activeProject ? allProjects.filter((p) => p.workspacePath === activeProject) : projects ?? [])
      .map((p) => ({ project: p, view: buildLibraryTree(p, keep) }))
      .filter(({ project: p, view }) => !q || view.shown > 0 || p.workspacePath === activeProject);
  }, [projects, allProjects, activeProject, q, bodyHits.slugs]);
  const shownCount = views.reduce((s, v) => s + v.view.shown, 0);

  const batchId = ingestBatchIdOf(selectedKey);
  const batchFolders = useMemo(() => {
    const ws = batches.find((b) => b.id === batchId)?.workspacePath;
    return (projects ?? []).find((p) => p.workspacePath === ws)?.folders ?? [];
  }, [batches, batchId, projects]);

  const selected = useMemo(() => {
    for (const p of projects ?? []) {
      const hit = p.items.find((e) => libraryEntryKey(e) === selectedKey);
      if (hit) return hit;
    }
    return null;
  }, [projects, selectedKey]);

  /** Run a schema/filing mutation with a spinner on `row`, then reload. A full
   *  reload rather than a splice: a rename re-keys docs and re-files artifacts
   *  server-side, so every count under it changes. */
  const mutate = useCallback(
    async (row: string, op: () => Promise<unknown>) => {
      setBusy(row);
      try {
        await op();
        setError(null);
        await load();
        void fetchDocs();
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(null);
      }
    },
    [load, fetchDocs],
  );

  const onCreateFolder = useCallback(
    (p: string, path: string) => {
      const parentPath = path.split("/").slice(0, -1).join("/");
      void mutate(libraryDropKey(p, parentPath), () =>
        invoke("library_create_folder", { workspacePath: p, path, description: null }),
      );
    },
    [mutate],
  );
  const onRenameFolder = useCallback(
    (p: string, from: string, to: string) =>
      void mutate(libraryDropKey(p, from), () =>
        invoke("library_rename_folder", { workspacePath: p, from, to }),
      ),
    [mutate],
  );
  const onDeleteFolder = useCallback(
    (p: string, path: string, label: string) => {
      if (!window.confirm(t("library.folder_delete_confirm", "删除文件夹「{{name}}」？", { name: label }))) return;
      void mutate(libraryDropKey(p, path), () =>
        invoke("library_delete_folder", { workspacePath: p, path }),
      );
    },
    [mutate, t],
  );
  const onDescribeFolder = useCallback(
    (p: string, path: string, description: string) =>
      void mutate(libraryDropKey(p, path), () =>
        invoke("library_describe_folder", { workspacePath: p, path, description }),
      ),
    [mutate],
  );

  const moveEntry = useCallback(
    (entry: LibraryEntry, p: string, folder: string) => {
      if ((entry.folder ?? "") === folder) return;
      void mutate(libraryDropKey(p, folder), async () => {
        const moved = await invoke<LibraryEntry>("library_move_item", {
          kind: entry.kind,
          id: entry.id,
          folder,
        });
        // A doc's slug — its id — changes with its folder; keep it selected.
        if (moved && selectedKey === libraryEntryKey(entry)) {
          set({ selectedKey: libraryEntryKey(moved) });
        }
      });
    },
    [mutate, selectedKey, set],
  );

  // ── Drag to file ──────────────────────────────────────────────────────────
  // Pointer events, not HTML5 DnD (inert in this webview — see usePointerDrag).
  const pressed = useRef<{ entry: LibraryEntry; project: string } | null>(null);
  const [dragging, setDragging] = useState<{ entry: LibraryEntry; project: string } | null>(null);
  const [dropAt, setDropAt] = useState<string | null>(null);
  const [dragPoint, setDragPoint] = useState<{ x: number; y: number } | null>(null);
  const endDrag = useCallback(() => {
    pressed.current = null;
    setDragging(null);
    setDropAt(null);
    setDragPoint(null);
  }, []);
  const drag = usePointerDrag({
    onStart: (pt) => {
      if (!pressed.current) return false;
      setDragging(pressed.current);
      setDragPoint({ x: pt.x, y: pt.y });
    },
    onMove: (pt) => {
      setDragPoint({ x: pt.x, y: pt.y });
      const key = dropTargetAt(pt.over, DROP_ATTR);
      const src = pressed.current;
      setDropAt(src && libraryDropTarget(key, src.project) !== null ? key : null);
    },
    onDrop: (pt) => {
      const src = pressed.current;
      const folder = src ? libraryDropTarget(dropTargetAt(pt.over, DROP_ATTR), src.project) : null;
      endDrag();
      if (src && folder !== null) moveEntry(src.entry, src.project, folder);
    },
    onCancel: endDrag,
  });

  const onOpen = useCallback(
    (e: LibraryEntry) => {
      // The click a finished drag fires is not an open.
      if (drag.didDrag()) return;
      set({ selectedKey: libraryEntryKey(e) });
    },
    [drag, set],
  );

  const treeCtx = (p: string): TreeCtx => ({
    project: p,
    selectedKey,
    dropActive: dropAt,
    busy,
    draggingKey: dragging ? libraryEntryKey(dragging.entry) : null,
    searching: !!q,
    onOpen,
    entryDrag: (e) => ({
      onPointerDown: (ev) => {
        pressed.current = { entry: e, project: p };
        drag.onPointerDown(ev);
      },
    }),
    onCreateFolder,
    onRenameFolder,
    onDeleteFolder,
    onDescribeFolder,
    onOpenBatch: (id) => set({ selectedKey: ingestBatchKey(id) }),
  });

  const list = (
    <div
      className={styles.list}
      onDragOver={onWebDragOver}
      onDragLeave={(e) => {
        if (e.currentTarget === e.target) setDropProject(null);
      }}
      onDrop={onWebDrop}
    >
      {projects === null ? (
        <SkeletonList rows={8} />
      ) : views.length === 0 ? (
        <div className={styles.none}>
          {projects.length === 0
            ? t("library.empty_upload", "还没有资料。agent 发布的文档和交付的文件会出现在这里；也可以在上方选一个项目，直接上传文件。")
            : t("library.no_match", "没有符合筛选条件的资料")}
        </div>
      ) : (
        views.map(({ project: p, view }) => (
          <ProjectBranch
            key={p.workspacePath}
            name={p.workspaceName}
            total={view.shown}
            folders={view.folders}
            unfiled={view.unfiled}
            ctx={treeCtx(p.workspacePath)}
            batches={batches.filter((b) => b.workspacePath === p.workspacePath)}
            uploadActive={dropProject === p.workspacePath}
            webUpload={webUpload?.project === p.workspacePath ? webUpload : null}
            onUpload={(kind) => onUpload(p.workspacePath, kind)}
          />
        ))
      )}
    </div>
  );

  return (
    <PageShell
      view="library"
      title={t("nav_section_library", "资料")}
      count={projects ? shownCount : null}
      countLoading={projects === null}
      search={{
        value: query,
        onChange: (v) => set({ query: v }),
        placeholder: t("library.search", "搜索标题 / 正文…"),
        busy: bodyHits.busy,
      }}
      subBar={
        <div className={styles.filters}>
          <select
            className={styles.select}
            value={activeProject}
            onChange={(e) => set({ project: e.target.value })}
            aria-label={t("library.project_all", "全部项目")}
          >
            <option value="">{t("library.project_all", "全部项目")}</option>
            {allProjects.map((p) => (
              <option key={p.workspacePath} value={p.workspacePath}>
                {p.workspaceName}
              </option>
            ))}
          </select>
          <span className={styles.hint}>
            {t("library.drag_hint_upload", "把文件、文件夹或压缩包拖到项目上即可上传，AI 自动归档；拖动条目到文件夹可手动归档")}
          </span>
        </div>
      }
      secondary={list}
      detailKey={selectedKey}
    >
      {error && <div className={styles.error}>{error}</div>}
      {dragging && dragPoint && (
        <div className={styles.drag_ghost} style={{ left: dragPoint.x + 12, top: dragPoint.y + 12 }}>
          {dragging.entry.title}
        </div>
      )}
      {batchId ? (
        <BatchDetail
          key={batchId}
          id={batchId}
          folders={batchFolders}
          onChanged={() => {
            void load();
            void loadBatches();
          }}
          onOpenArtifact={(id) => set({ selectedKey: `artifact:${id}` })}
          onClose={() => set({ selectedKey: null })}
        />
      ) : selected ? (
        <LibraryDetail key={libraryEntryKey(selected)} item={selected} onClose={() => set({ selectedKey: null })} />
      ) : (
        <EmptyState
          icon={<Library size={28} strokeWidth={1.4} />}
          title={t("library.pick_title", "选一份资料")}
          subtitle={t(
            "library.pick_sub_upload",
            "文档和产出按项目的目录结构放在一起。把文件拖到项目上即可上传，AI 会读内容、写摘要、放进合适的目录。",
          )}
        />
      )}
    </PageShell>
  );
}

/** Batch rows shown under a project: every running one, then the most recent
 *  finished ones (the rest stay reachable once dismissed ones go). */
const RECENT_BATCHES = 3;

function ProjectBranch({
  name,
  total,
  folders,
  unfiled,
  ctx,
  batches,
  uploadActive,
  webUpload,
  onUpload,
}: {
  name: string;
  total: number;
  folders: LibraryFolderNode[];
  unfiled: LibraryEntry[];
  ctx: TreeCtx;
  batches: IngestBatchSummary[];
  uploadActive: boolean;
  webUpload: { sent: number; total: number } | null;
  onUpload: (kind: PickKind) => void;
}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
  const [creating, setCreating] = useState(false);
  const rootBusy = ctx.busy === libraryDropKey(ctx.project, "") && !creating;
  const running = batches.filter((b) => isIngestRunning(b.status));
  const shownBatches = [
    ...running,
    ...batches.filter((b) => !isIngestRunning(b.status)).slice(0, Math.max(0, RECENT_BATCHES - running.length)),
  ];
  return (
    <div
      className={`${styles.project}${uploadActive ? ` ${styles.project_drop}` : ""}`}
      {...{ [UPLOAD_ATTR]: ctx.project }}
    >
      {uploadActive && (
        <div className={styles.drop_banner}>
          {t("library.drop_here", "松开即上传到「{{name}}」", { name })}
        </div>
      )}
      <div className={styles.tree_row} style={{ paddingLeft: 4 }}>
        <button
          type="button"
          className={styles.twisty}
          aria-label={expanded ? "Collapse" : "Expand"}
          onClick={() => setExpanded((v) => !v)}
        >
          {expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        </button>
        <span className={styles.tree_target} title={ctx.project}>
          <FolderGit2 size={15} strokeWidth={1.5} />
          <span className={styles.project_name}>{name}</span>
          <span className={styles.count}>{total}</span>
        </span>
        <span className={styles.actions}>
          <button
            type="button"
            className={styles.action}
            title={t("library.folder_new", "新建文件夹")}
            aria-label={t("library.folder_new", "新建文件夹")}
            onClick={() => {
              setExpanded(true);
              setCreating(true);
            }}
          >
            <FolderPlus size={13} strokeWidth={1.5} />
          </button>
        </span>
        <span className={styles.upload_actions}>
          <button
            type="button"
            className={styles.upload_btn}
            title={t("library.upload_files_hint", "上传文件或压缩包，AI 自动归档")}
            onClick={() => onUpload("files")}
            disabled={webUpload !== null}
          >
            <FileUp size={13} strokeWidth={1.6} />
            {t("library.upload_files", "上传")}
          </button>
          <button
            type="button"
            className={styles.upload_btn}
            title={t("library.upload_folder", "上传文件夹")}
            aria-label={t("library.upload_folder", "上传文件夹")}
            onClick={() => onUpload("folder")}
            disabled={webUpload !== null}
          >
            <FolderUp size={13} strokeWidth={1.6} />
          </button>
        </span>
      </div>
      {creating && (
        <InlineInput
          depth={0}
          placeholder={t("library.folder_name_placeholder", "文件夹名")}
          initial=""
          onDone={(v) => {
            setCreating(false);
            if (v) ctx.onCreateFolder(ctx.project, v);
          }}
        />
      )}
      {expanded && (
        <>
          {webUpload && (
            <div className={styles.row_busy}>
              <Spinner size={12} />
              {t("library.browser_uploading", "正在上传 {{sent}}/{{total}}", webUpload)}
            </div>
          )}
          {shownBatches.map((b) => (
            <BatchRow
              key={b.id}
              batch={b}
              depth={1}
              active={ctx.selectedKey === ingestBatchKey(b.id)}
              onOpen={() => ctx.onOpenBatch(b.id)}
            />
          ))}
          {rootBusy && <div className={styles.row_busy}><Spinner size={12} /></div>}
          {folders.map((f) => (
            <FolderBranch key={f.path} node={f} depth={1} ctx={ctx} />
          ))}
          {folders.length === 0 && !ctx.searching && (
            <div className={styles.no_folders}>
              {t("library.no_folders_upload", "这个项目还没有目录结构。点 + 新建文件夹，或直接上传文件——AI 会按项目内容起草一套。")}
            </div>
          )}
          <UnfiledBranch items={unfiled} ctx={ctx} />
        </>
      )}
    </div>
  );
}

function FolderBranch({ node, depth, ctx }: { node: LibraryFolderNode; depth: number; ctx: TreeCtx }) {
  const { t } = useTranslation();
  // Folders start open while searching (so every hit is visible) and when
  // shallow; deeper ones start closed so a big schema reads at a glance.
  const [expanded, setExpanded] = useState(depth <= 1);
  const [editing, setEditing] = useState<null | "create" | "rename" | "describe">(null);
  const key = libraryDropKey(ctx.project, node.path);
  const open = expanded || ctx.searching;
  const rowBusy = ctx.busy === key;

  const commit = (value: string) => {
    const mode = editing;
    setEditing(null);
    if (mode === "describe") {
      if (value !== node.description) ctx.onDescribeFolder(ctx.project, node.path, value);
      return;
    }
    if (!value) return;
    if (mode === "create") {
      setExpanded(true);
      ctx.onCreateFolder(ctx.project, `${node.path}/${value}`);
      return;
    }
    // Rename replaces the last segment only; re-nesting is a different gesture.
    if (value === node.label) return;
    const parent = node.path.split("/").slice(0, -1).join("/");
    ctx.onRenameFolder(ctx.project, node.path, parent ? `${parent}/${value}` : value);
  };

  return (
    <div>
      <div
        className={`${styles.tree_row} ${ctx.dropActive === key ? styles.tree_row_drop : ""}`}
        style={{ paddingLeft: 4 + depth * 14 }}
        {...{ [DROP_ATTR]: key }}
      >
        <button
          type="button"
          className={styles.twisty}
          aria-label={open ? "Collapse" : "Expand"}
          onClick={() => setExpanded((v) => !v)}
        >
          {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        </button>
        <button type="button" className={styles.tree_target} onClick={() => setExpanded((v) => !v)}>
          {open ? <FolderOpen size={15} strokeWidth={1.4} /> : <Folder size={15} strokeWidth={1.4} />}
          <span className={styles.folder_text}>
            <span className={styles.folder_label} title={node.path}>{node.label}</span>
            {node.description && (
              <span className={styles.folder_desc} title={node.description}>{node.description}</span>
            )}
          </span>
          {rowBusy ? <Spinner size={12} /> : <span className={styles.count}>{node.total}</span>}
        </button>
        <span className={styles.actions}>
          <button
            type="button"
            className={styles.action}
            title={t("library.folder_new_sub", "新建子文件夹")}
            aria-label={t("library.folder_new_sub", "新建子文件夹")}
            onClick={() => setEditing("create")}
          >
            <FolderPlus size={13} strokeWidth={1.5} />
          </button>
          <button
            type="button"
            className={styles.action}
            title={t("library.folder_describe", "编辑说明")}
            aria-label={t("library.folder_describe", "编辑说明")}
            onClick={() => setEditing("describe")}
          >
            <NotebookPen size={13} strokeWidth={1.5} />
          </button>
          <button
            type="button"
            className={styles.action}
            title={t("library.folder_rename", "重命名")}
            aria-label={t("library.folder_rename", "重命名")}
            onClick={() => setEditing("rename")}
          >
            <Pencil size={13} strokeWidth={1.5} />
          </button>
          <button
            type="button"
            className={styles.action}
            title={t("library.folder_delete", "删除文件夹")}
            aria-label={t("library.folder_delete", "删除文件夹")}
            onClick={() => ctx.onDeleteFolder(ctx.project, node.path, node.label)}
          >
            <Trash2 size={13} strokeWidth={1.5} />
          </button>
        </span>
      </div>
      {editing && (
        <InlineInput
          depth={depth}
          initial={editing === "rename" ? node.label : editing === "describe" ? node.description : ""}
          placeholder={
            editing === "describe"
              ? t("library.folder_desc_placeholder", "一句话说明这里放什么")
              : t("library.folder_name_placeholder", "文件夹名")
          }
          // A description may be cleared on purpose; a name may not.
          allowEmpty={editing === "describe"}
          onDone={(v) => (v === null ? setEditing(null) : commit(v))}
        />
      )}
      {open && (
        <>
          {node.children.map((c) => (
            <FolderBranch key={c.path} node={c} depth={depth + 1} ctx={ctx} />
          ))}
          {node.items.map((e) => (
            <EntryRow key={libraryEntryKey(e)} entry={e} depth={depth + 1} ctx={ctx} />
          ))}
        </>
      )}
    </div>
  );
}

function UnfiledBranch({ items, ctx }: { items: LibraryEntry[]; ctx: TreeCtx }) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(true);
  const key = libraryDropKey(ctx.project, "");
  if (items.length === 0 && ctx.searching) return null;
  return (
    <div>
      <div
        className={`${styles.tree_row} ${styles.unfiled_row} ${ctx.dropActive === key ? styles.tree_row_drop : ""}`}
        style={{ paddingLeft: 18 }}
        {...{ [DROP_ATTR]: key }}
      >
        <button
          type="button"
          className={styles.twisty}
          aria-label={expanded ? "Collapse" : "Expand"}
          onClick={() => setExpanded((v) => !v)}
          disabled={items.length === 0}
        >
          {items.length > 0 ? (expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} />) : null}
        </button>
        <button type="button" className={styles.tree_target} onClick={() => setExpanded((v) => !v)}>
          <Inbox size={15} strokeWidth={1.4} />
          <span className={styles.folder_text}>
            <span className={styles.folder_label}>{t("library.unfiled", "未归档")}</span>
            <span className={styles.folder_desc}>
              {t("library.unfiled_desc", "不在任何目录里的资料，拖进文件夹即可归档")}
            </span>
          </span>
          <span className={styles.count}>{items.length}</span>
        </button>
      </div>
      {expanded &&
        items.map((e) => <EntryRow key={libraryEntryKey(e)} entry={e} depth={2} ctx={ctx} showLocation />)}
    </div>
  );
}

function EntryRow({
  entry,
  depth,
  ctx,
  showLocation = false,
}: {
  entry: LibraryEntry;
  depth: number;
  ctx: TreeCtx;
  showLocation?: boolean;
}) {
  const { t } = useTranslation();
  const key = libraryEntryKey(entry);
  const active = ctx.selectedKey === key;
  const Icon = entry.kind === "doc" ? BookOpen : (ARTIFACT_ICON[entry.artifactKind] ?? FileText);
  return (
    <button
      type="button"
      className={`${styles.row}${active ? ` ${styles.row_active}` : ""}${
        ctx.draggingKey === key ? ` ${styles.row_dragging}` : ""
      }`}
      style={{ paddingLeft: 8 + depth * 14 }}
      onClick={() => ctx.onOpen(entry)}
      aria-current={active ? "true" : undefined}
      {...ctx.entryDrag(entry)}
    >
      <span className={styles.icon} data-type={entry.kind}>
        <Icon size={14} strokeWidth={1.6} />
      </span>
      <span className={styles.body}>
        <span className={styles.title}>{entry.title}</span>
        <span className={styles.meta}>
          <span>{entry.kind === "doc" ? t("library.type_doc", "文档") : t("library.type_artifact", "产出")}</span>
          {showLocation && entry.location && (
            <span className={styles.location} title={entry.location}>
              {entry.location}/
            </span>
          )}
          {entry.updatedMs > 0 && <span className={styles.time}>{timeAgo(entry.updatedMs, t)}</span>}
        </span>
      </span>
    </button>
  );
}

/** Inline editor under a tree row. Enter or blur commits, Escape cancels
 *  (`onDone(null)`). Inline rather than `window.prompt`, which is not reliably
 *  available in the Tauri webview. */
function InlineInput({
  depth,
  initial,
  placeholder,
  allowEmpty = false,
  onDone,
}: {
  depth: number;
  initial: string;
  placeholder: string;
  allowEmpty?: boolean;
  onDone: (value: string | null) => void;
}) {
  const done = useRef(false);
  const finish = (v: string | null) => {
    if (done.current) return;
    done.current = true;
    const trimmed = v === null ? null : v.trim();
    onDone(trimmed === "" && !allowEmpty ? null : trimmed);
  };
  return (
    <input
      className={styles.tree_input}
      style={{ marginLeft: 26 + depth * 14 }}
      autoFocus
      defaultValue={initial}
      placeholder={placeholder}
      onKeyDown={(e) => {
        if (e.key === "Enter") finish(e.currentTarget.value);
        if (e.key === "Escape") finish(null);
      }}
      onBlur={(e) => finish(e.currentTarget.value)}
    />
  );
}

/** The reader for the selected item: the side-rail pane of its kind, pinned
 *  open. Its close / collapse both clear the selection; there are no sibling
 *  cards here, so the "close others" family has nothing to act on. */
function LibraryDetail({ item, onClose }: { item: LibraryEntry; onClose: () => void }) {
  const setSelected = useUIStore((s) => s.updateMainViewState);
  const doc: AuxDoc =
    item.kind === "doc"
      ? { id: `wiki:${item.id}`, kind: "wiki", ref: item.id, label: item.title, openedMs: 0 }
      : { id: `artifact:${item.id}`, kind: "artifact", ref: item.id, label: item.title, openedMs: 0 };
  const tail: AuxCardTail = {
    isExpanded: true,
    onToggle: onClose,
    onClose,
    onCloseOthers: () => {},
    onCloseAll: onClose,
    onHideRail: onClose,
    otherCount: 0,
  };
  return (
    <div className={styles.detail}>
      {doc.kind === "wiki" ? (
        <WikiTabPane
          doc={doc}
          tail={tail}
          onOpenSlug={(slug) => setSelected("library", { selectedKey: `doc:${slug}` })}
        />
      ) : (
        <ArtifactTabPane doc={doc} tail={tail} />
      )}
    </div>
  );
}
