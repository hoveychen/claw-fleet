import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Library, ScanText, Square } from "lucide-react";

import { useUIStore } from "../store";
import { getItem, setItem } from "../storage";
import { useWikiDocsStore } from "../hooks/useWikiDocs";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { useFadeOnChange } from "../hooks/useFadeOnChange";
import { dropTargetAt, usePointerDrag } from "../hooks/usePointerDrag";
import {
  buildLibraryTree,
  libraryDropKey,
  libraryDropTarget,
  libraryEntryKey,
  type LibraryEntry,
  type LibraryProjectTree,
} from "../../../shared-ts/libraryTree";
import {
  defaultLibraryProject,
  findLibraryFolder,
  orderLibraryProjects,
  projectTotals,
  type LibrarySort,
  type LibraryTypeGroup,
} from "../../../shared-ts/libraryBrowse";
import {
  ingestBatchIdOf,
  ingestBatchKey,
  isIngestRunning,
  isRefileBatch,
  type BackfillStatus,
  type IngestBatch,
  type IngestBatchSummary,
  type RefilePlan,
} from "../../../shared-ts/libraryIngest";
import type { LibraryHit } from "../../../shared-ts/librarySearch";
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
import { isWebBuild } from "../hostEnv";
import { formatBytes } from "../formatBytes";
import { exportLibraryZip, type LibraryExportSpec } from "../libraryExport";
import type { RegisteredProject } from "../projectList";
import type { AuxCardTail } from "./auxDocMenu";
import { ArtifactTabPane } from "./ArtifactTabPane";
import { BATCH_POLL_MS, BatchDetail } from "./LibraryBatch";
import { LibraryContent, type ContentMode, type ExportRequest } from "./LibraryContent";
import { DROP_ATTR, LibraryFolderRail, UPLOAD_ATTR } from "./LibraryFolderRail";
import { WikiTabPane } from "./WikiTabPane";
import { EmptyState } from "./EmptyState";
import { PageShell } from "./PageShell";
import { ResizeHandle } from "./ResizeHandle";
import { SkeletonList, Spinner } from "./loading";
import styles from "./LibraryView.module.css";

/** Where the page reopens: the project it last showed. */
const LAST_PROJECT_KEY = "library-last-project";

/** Items the outline keeps per folder: the overview cards list three. */
const OUTLINE_RECENT = 3;

function uploadTargetAt(el: Element | null): string | null {
  return el?.closest(`[${UPLOAD_ATTR}]`)?.getAttribute(UPLOAD_ATTR) ?? null;
}

/**
 * 资料 (Library): wiki documents and artifacts by project and folder.
 *
 * Three columns: the left rail lists the projects and, under the open one,
 * its folder tree with counts; the middle column shows the open folder's items
 * as cards (summary, tags, thumbnail), the project overview (one card per
 * folder) when no folder is picked, search results in the same cards, or the
 * upload / re-file history; the right column previews the picked item with
 * the reader panes a conversation's side rail opens, and is absent until
 * something is picked.
 *
 * The folders are the project's schema (`claw_fleet_core::library`); moving
 * an item re-keys a doc's slug or sets an artifact's `path`, done server-side.
 *
 * Loaded level by level: the rail and the overview come from an outline
 * (folders, counts, the newest few items per folder, no summaries), and a
 * folder's items with their summaries are fetched when it is opened. The full
 * tree of a real library is ~1.4 MB, most of it summaries nobody is looking at.
 */
export function LibraryView() {
  const { t } = useTranslation();
  const lib = useUIStore((s) => s.mainViewState.library);
  const { query, project, selectedKey, folder, sort, searchAll } = lib;
  const showBatches = lib.batches;
  const typeGroup = lib.typeGroup as LibraryTypeGroup | null;
  const update = useUIStore((s) => s.updateMainViewState);
  const set = useCallback((patch: Partial<typeof lib>) => update("library", patch), [update]);

  const fetchDocs = useWikiDocsStore((s) => s.fetch);
  const [projects, setProjects] = useState<LibraryProjectTree[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  // Bumped by every reload, so the open folder's items are refetched with it.
  const [rev, setRev] = useState(0);

  const load = useCallback(async () => {
    try {
      setProjects(
        (await invoke<LibraryProjectTree[]>("library_tree", {
          workspace: null,
          outline: true,
          recent: OUTLINE_RECENT,
        })) ?? [],
      );
      setRev((r) => r + 1);
    } catch (e) {
      setProjects((prev) => prev ?? []);
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  // Registered projects that have no library yet: listed in the rail so a
  // first upload has somewhere explicit to go.
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
      .map((r) => ({ workspacePath: r.path, workspaceName: r.name, folders: [], items: [] }));
    return orderLibraryProjects([...tree, ...extra]);
  }, [projects, registered]);

  // The open project: the one picked, else the last one shown, else the most
  // recently active. Never "every project at once".
  const activeProject = useMemo(
    () => (projects === null ? "" : defaultLibraryProject(allProjects, project || getItem(LAST_PROJECT_KEY))),
    [projects, allProjects, project],
  );
  useEffect(() => {
    if (activeProject) setItem(LAST_PROJECT_KEY, activeProject);
  }, [activeProject]);
  const current = useMemo(
    () => allProjects.find((p) => p.workspacePath === activeProject) ?? null,
    [allProjects, activeProject],
  );
  const view = useMemo(() => (current ? buildLibraryTree(current) : null), [current]);
  const projectName = useCallback(
    (ws: string) => allProjects.find((p) => p.workspacePath === ws)?.workspaceName ?? ws.split("/").pop() ?? ws,
    [allProjects],
  );

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
  const projectBatches = useMemo(
    () => batches.filter((b) => b.workspacePath === activeProject),
    [batches, activeProject],
  );
  const projectRunning = projectBatches.filter((b) => isIngestRunning(b.status));

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
      if (!isWebBuild()) {
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

  // ── Re-filing a project's unfiled items ───────────────────────────────────
  const [refilePending, setRefilePending] = useState<string | null>(null);
  const onRefile = useCallback(
    async (p: string) => {
      setRefilePending(p);
      try {
        const plan = await invoke<RefilePlan>("library_refile_plan", { workspace: p });
        if (plan.runningBatch) {
          set({ selectedKey: ingestBatchKey(plan.runningBatch) });
          return;
        }
        const n = plan.unfiledDocs + plan.unfiledArtifacts;
        if (n === 0) {
          setError(t("library.refile_nothing", "「{{name}}」没有未归档的资料", { name: plan.workspaceName }));
          return;
        }
        const healthy = plan.health === "ok";
        const question = [
          t("library.refile_confirm", "让 AI 整理「{{name}}」的 {{n}} 条未归档资料（文档 {{docs}} 篇、产出 {{artifacts}} 个），放进合适的目录？", {
            name: plan.workspaceName,
            n,
            docs: plan.unfiledDocs,
            artifacts: plan.unfiledArtifacts,
          }),
          plan.willDraft
            ? t("library.refile_confirm_draft", "这个项目还没有目录结构，AI 会先按项目内容起草一套。")
            : "",
          t("library.refile_confirm_cost", "预计调用 AI {{calls}} 次，约 ${{cost}}。整理结果可以逐条或整批撤销；文档换了位置，旧链接照样能打开。", {
            calls: plan.calls,
            cost: plan.estCostUsd.toFixed(2),
          }),
          healthy ? "" : t(`library.refile_health_${plan.health}`, plan.healthReason),
        ]
          .filter(Boolean)
          .join("\n\n");
        if (!window.confirm(question)) return;
        // The user saw the health warning and confirmed anyway.
        afterStart(await invoke<IngestBatch>("library_refile_start", { workspacePath: p, force: !healthy }));
      } catch (e) {
        setError(String(e));
      } finally {
        setRefilePending(null);
      }
    },
    [afterStart, set, t],
  );

  // ── Dropping files from the OS ────────────────────────────────────────────
  // The target is the project row under the pointer, else the open project.
  const [dropProject, setDropProject] = useState<string | null>(null);
  const dropRefs = useRef({ activeProject, uploadPaths, uploadWebFiles });
  dropRefs.current = { activeProject, uploadPaths, uploadWebFiles };
  useEffect(() => {
    if (isWebBuild()) return;
    let unlisten: (() => void) | undefined;
    let disposed = false;
    const targetAt = (pos: { x: number; y: number }) => {
      const dpr = window.devicePixelRatio > 0 ? window.devicePixelRatio : 1;
      return (
        uploadTargetAt(document.elementFromPoint(pos.x / dpr, pos.y / dpr)) ||
        dropRefs.current.activeProject ||
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
            if (p.paths?.length && target) void dropRefs.current.uploadPaths(target, p.paths);
          }
        });
        if (disposed) un();
        else unlisten = un;
      } catch {
        // Not a Tauri webview: the HTML5 handlers on the page take drops.
      }
    })();
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);
  const webDrop = {
    onDragOver: (e: React.DragEvent) => {
      if (!isWebBuild() || !e.dataTransfer.types.includes("Files")) return;
      e.preventDefault();
      setDropProject(uploadTargetAt(e.target as Element) || activeProject || null);
    },
    onDragLeave: (e: React.DragEvent) => {
      if (e.currentTarget === e.target) setDropProject(null);
    },
    onDrop: (e: React.DragEvent) => {
      if (!isWebBuild() || !e.dataTransfer.types.includes("Files")) return;
      e.preventDefault();
      const target = uploadTargetAt(e.target as Element) || activeProject;
      setDropProject(null);
      if (!target) return;
      // Entries are read synchronously inside; the walk itself is async.
      void filesFromDataTransfer(e.dataTransfer).then((files) => uploadWebFiles(target, files));
    },
  };

  const search = useLibraryHits(query, searchAll ? "" : activeProject);
  const q = query.trim();
  // Two characters and up, the content column becomes the index's ranked
  // hits; a single character filters the open folder on titles.
  const searching = q.length >= 2;

  // A folder deleted or renamed away falls back to the overview.
  const folderExists = folder === null || folder === "" || (!!view && !!findLibraryFolder(view.folders, folder));
  const openFolder = folderExists ? folder : null;
  const mode: ContentMode = searching ? "search" : showBatches ? "batches" : openFolder === null ? "overview" : "folder";
  const folderItems = useFolderItems(activeProject, mode === "folder" ? openFolder : null, rev, setError);

  // Every entry loaded so far by key — the outline's few per folder, the open
  // folder's, the search hits — and the project (core's key, not the item's
  // own possibly-worktree path) each belongs to, which a drop is checked
  // against.
  const { entryByKey, projectOf } = useMemo(() => {
    const entries = new Map<string, LibraryEntry>();
    const owners = new Map<string, string>();
    const put = (e: LibraryEntry, project: string) => {
      const key = libraryEntryKey(e);
      // The fuller record wins: a folder listing carries the summary and tags
      // an outline strips.
      if (!entries.has(key) || e.summary !== undefined) entries.set(key, e);
      owners.set(key, project);
    };
    for (const h of search.hits ?? []) put(hitEntry(h), h.project);
    for (const p of projects ?? []) for (const e of p.items) put(e, p.workspacePath);
    for (const e of folderItems ?? []) put(e, activeProject);
    return { entryByKey: entries, projectOf: owners };
  }, [projects, folderItems, activeProject, search.hits]);

  const batchId = ingestBatchIdOf(selectedKey);
  const batchFolders = useMemo(() => {
    const ws = batches.find((b) => b.id === batchId)?.workspacePath;
    return (projects ?? []).find((p) => p.workspacePath === ws)?.folders ?? [];
  }, [batches, batchId, projects]);

  const selected = useMemo<SelectedItem | null>(() => {
    if (!selectedKey) return null;
    const hit = entryByKey.get(selectedKey);
    if (hit) return hit;
    // An item in a folder that is not loaded (a link from a doc, a batch
    // row) still opens: the reader panes fetch the item by id themselves.
    return selectedFromKey(selectedKey);
  }, [entryByKey, selectedKey]);

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
      void mutate(libraryDropKey(p, from), async () => {
        await invoke("library_rename_folder", { workspacePath: p, from, to });
        // Keep showing the folder under its new name.
        if (folder === from) set({ folder: to });
        else if (folder?.startsWith(`${from}/`)) set({ folder: `${to}${folder.slice(from.length)}` });
      }),
    [mutate, folder, set],
  );
  const onDeleteFolder = useCallback(
    (p: string, path: string, label: string) => {
      if (!window.confirm(t("library.folder_delete_confirm", "删除文件夹「{{name}}」？", { name: label }))) return;
      void mutate(libraryDropKey(p, path), () => invoke("library_delete_folder", { workspacePath: p, path }));
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
  const onAcceptFolder = useCallback(
    (p: string, path: string) =>
      void mutate(libraryDropKey(p, path), () => invoke("library_accept_folder", { workspacePath: p, path })),
    [mutate],
  );

  const moveEntry = useCallback(
    (entry: LibraryEntry, p: string, to: string) => {
      if ((entry.folder ?? "") === to) return;
      void mutate(libraryDropKey(p, to), async () => {
        const moved = await invoke<LibraryEntry>("library_move_item", {
          kind: entry.kind,
          id: entry.id,
          folder: to,
        });
        // A doc's slug — its id — changes with its folder; keep it selected.
        if (moved && selectedKey === libraryEntryKey(entry)) {
          set({ selectedKey: libraryEntryKey(moved) });
        }
      });
    },
    [mutate, selectedKey, set],
  );

  // ── Export to a zip ───────────────────────────────────────────────────────
  // One at a time: `exporting` is the key of the export running (a folder
  // path, "selection" or "project"), so its button shows the spinner.
  const [exporting, setExporting] = useState<string | null>(null);
  const exportGuard = useRef(false);
  const onExport = useCallback(
    async (req: ExportRequest) => {
      if (exportGuard.current || !activeProject) return;
      exportGuard.current = true;
      setExporting(req.key);
      try {
        const spec: LibraryExportSpec = {
          workspacePath: activeProject,
          scope: req.scope,
          folder: req.folder,
          items: req.items,
          unfiledLabel: t("library.unfiled", "未归档"),
        };
        const done = await exportLibraryZip(spec, `${req.name}.zip`);
        if (done === null) return;
        setError(null);
        if (done === "download") {
          setNotice(t("library.export_started", "已开始下载「{{name}}」，进度见浏览器的下载栏", { name: `${req.name}.zip` }));
        } else {
          const text = t("library.export_done", "已导出 {{items}} 条资料（{{files}} 个文件，{{size}}）", {
            items: done.itemCount,
            files: done.fileCount,
            size: formatBytes(done.totalBytes),
          });
          setNotice(
            done.skipped.length === 0
              ? text
              : `${text} · ${t("library.export_skipped", "跳过 {{n}} 条（存储的文件已不在）：{{names}}", {
                  n: done.skipped.length,
                  names: done.skipped.join("、"),
                })}`,
          );
        }
      } catch (e) {
        setError(t("library.export_failed", "导出失败：{{error}}", { error: String(e) }));
      } finally {
        setExporting(null);
        exportGuard.current = false;
      }
    },
    [activeProject, t],
  );

  // ── Drag a card onto a rail folder to file it ─────────────────────────────
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
      const to = src ? libraryDropTarget(dropTargetAt(pt.over, DROP_ATTR), src.project) : null;
      endDrag();
      if (src && to !== null) moveEntry(src.entry, src.project, to);
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

  const pickFolder = useCallback(
    (path: string | null) => set({ folder: path, batches: false, query: "", typeGroup: null }),
    [set],
  );
  const pickProject = useCallback(
    (p: string) => {
      if (p === activeProject) {
        pickFolder(null);
        return;
      }
      set({ project: p, folder: null, batches: false, selectedKey: null, typeGroup: null });
    },
    [activeProject, pickFolder, set],
  );

  // Stable: the backfill button refetches its status whenever this changes,
  // which as an inline arrow was on every render (each pointer move of a drag).
  const onBackfillFinished = useCallback(() => {
    void load();
    void fetchDocs();
  }, [load, fetchDocs]);

  // ── Preview column ────────────────────────────────────────────────────────
  const preview = batchId ? (
    <BatchDetail
      key={batchId}
      id={batchId}
      folders={batchFolders}
      onChanged={() => {
        void load();
        void loadBatches();
      }}
      onOpenArtifact={(id) => set({ selectedKey: `artifact:${id}` })}
      onOpenDoc={(slug) => set({ selectedKey: `doc:${slug}` })}
      onClose={() => set({ selectedKey: null })}
    />
  ) : selected ? (
    <LibraryDetail key={libraryEntryKey(selected)} item={selected} onClose={() => set({ selectedKey: null })} />
  ) : null;
  const previewRef = useRef<HTMLDivElement>(null);
  useFadeOnChange(previewRef, preview ? selectedKey : null);
  const { width: listWidth, isDragging: resizing, onMouseDown: onResize } = useResizableWidth(
    "library-list-width",
    { min: 300, max: 760, initial: 420 },
  );

  const dropName = dropProject ? projectName(dropProject) : "";

  const rail = (
    <div className={styles.list} {...webDrop}>
      {projects === null ? (
        <SkeletonList rows={8} />
      ) : allProjects.length === 0 ? (
        <div className={styles.none}>
          {t("library.empty_upload", "还没有资料。agent 发布的文档和交付的文件会出现在这里；也可以在上方选一个项目，直接上传文件。")}
        </div>
      ) : (
        <LibraryFolderRail
          projects={allProjects}
          active={activeProject}
          view={view}
          batchCount={projectBatches.length}
          batchesRunning={projectRunning.length}
          showBatches={showBatches && !searching}
          dropProject={dropProject}
          onPickProject={pickProject}
          onShowBatches={() => set({ batches: true, query: "" })}
          ctx={{
            project: activeProject,
            folder: openFolder,
            contentElsewhere: searching || showBatches,
            dropActive: dropAt,
            busy,
            onPickFolder: pickFolder,
            onCreateFolder,
            onRenameFolder,
            onDeleteFolder,
            onDescribeFolder,
            onAcceptFolder,
          }}
        />
      )}
    </div>
  );

  return (
    <PageShell
      view="library"
      title={t("nav_section_library", "资料")}
      sectionSwitch={false}
      count={current ? projectTotals(current).total : null}
      countLoading={projects === null}
      search={{
        value: query,
        onChange: (v) => set({ query: v }),
        placeholder: t("library.search_full", "搜索标题 / 正文 / 摘要 / 标签…"),
        busy: search.busy,
      }}
      subBar={
        <BackfillButton onFinished={onBackfillFinished} onError={setError} />
      }
      secondary={rail}
    >
      <div className={styles.split} {...webDrop}>
        <section
          className={preview ? `${styles.content} ${styles.content_with_preview}` : styles.content}
          style={preview ? { width: listWidth } : undefined}
        >
          {error && (
            <div className={styles.error} onClick={() => setError(null)}>
              {error}
            </div>
          )}
          {notice && (
            <div className={styles.notice} onClick={() => setNotice(null)} data-library-notice>
              {notice}
            </div>
          )}
          {dropProject && (
            <div className={styles.drop_banner}>{t("library.drop_here", "松开即上传到「{{name}}」", { name: dropName })}</div>
          )}
          {projects !== null && allProjects.length === 0 && !searching ? (
            <EmptyState
              icon={<Library size={28} strokeWidth={1.4} />}
              title={t("library.pick_title", "选一份资料")}
              subtitle={t(
                "library.pick_sub_upload",
                "文档和产出按项目的目录结构放在一起。把文件拖到项目上即可上传，AI 会读内容、写摘要、放进合适的目录。",
              )}
            />
          ) : (
            <LibraryContent
              mode={mode}
              project={current}
              view={view}
              folder={openFolder}
              folderItems={folderItems}
              query={searching ? query : q}
              typeGroup={typeGroup}
              sort={sort as LibrarySort}
              onTypeGroup={(g) => set({ typeGroup: g })}
              onSort={(s) => set({ sort: s })}
              onPickFolder={pickFolder}
              onAcceptFolder={onAcceptFolder}
              hits={search.hits}
              searchAll={searchAll || !current}
              onSearchAll={(all) => set({ searchAll: all })}
              entryByKey={entryByKey}
              projectName={projectName}
              batches={projectBatches}
              onOpenBatch={(id) => set({ selectedKey: ingestBatchKey(id) })}
              narrow={!!preview}
              ctx={{
                selectedKey,
                draggingKey: dragging ? libraryEntryKey(dragging.entry) : null,
                onOpen,
                entryDrag: (e) => ({
                  onPointerDown: (ev) => {
                    const project = projectOf.get(libraryEntryKey(e));
                    if (!project) return;
                    pressed.current = { entry: e, project };
                    drag.onPointerDown(ev);
                  },
                }),
              }}
              verbs={{
                onUpload: (kind) => activeProject && onUpload(activeProject, kind),
                uploading: webUpload?.project === activeProject ? webUpload : null,
                onRefile: () => void onRefile(activeProject),
                refiling: projectRunning.some(isRefileBatch) || refilePending === activeProject,
                onExport: (req) => void onExport(req),
                exporting,
              }}
            />
          )}
          {preview && <ResizeHandle side="left" active={resizing} onMouseDown={onResize} />}
        </section>
        {preview && (
          <section ref={previewRef} className={styles.preview}>
            {preview}
          </section>
        )}
      </div>
      {dragging && dragPoint && (
        <div className={styles.drag_ghost} style={{ left: dragPoint.x + 12, top: dragPoint.y + 12 }}>
          {dragging.entry.title}
        </div>
      )}
    </PageShell>
  );
}

/** The open folder's items and everything beneath it, with summaries and tags
 *  (`library_folder_items`; `folder` `""` = unfiled, `null` = none open).
 *  Null until the first answer for this folder; a refetch after a reload
 *  (`rev`) keeps the previous list on screen instead of blanking it. */
function useFolderItems(
  project: string,
  folder: string | null,
  rev: number,
  onError: (e: string) => void,
): LibraryEntry[] | null {
  const [state, setState] = useState<{ key: string; items: LibraryEntry[] } | null>(null);
  const key = folder === null || !project ? "" : `${project}\u0000${folder}`;
  useEffect(() => {
    if (!key || folder === null) return;
    let alive = true;
    invoke<LibraryEntry[]>("library_folder_items", { workspacePath: project, folder })
      .then((items) => {
        if (alive) setState({ key, items: items ?? [] });
      })
      .catch((e) => {
        if (!alive) return;
        setState({ key, items: [] });
        onError(String(e));
      });
    return () => {
      alive = false;
    };
  }, [key, project, folder, rev, onError]);
  return key && state?.key === key ? state.items : null;
}

/** A search hit as a list entry, with the file facts its thumbnail needs. */
function hitEntry(h: LibraryHit): LibraryEntry {
  return {
    kind: h.kind === "doc" ? "doc" : "artifact",
    id: h.id,
    title: h.title,
    workspacePath: h.workspacePath,
    folder: h.folder || null,
    location: h.location,
    updatedMs: h.updatedMs,
    artifactKind: h.artifactKind,
    name: h.name,
    mime: h.mime,
    sizeBytes: h.sizeBytes,
  };
}

/** `doc:<slug>` / `artifact:<id>` as something the preview can open. */
function selectedFromKey(key: string): SelectedItem | null {
  const cut = key.indexOf(":");
  if (cut < 0) return null;
  const kind = key.slice(0, cut);
  const id = key.slice(cut + 1);
  return (kind === "doc" || kind === "artifact") && id ? { kind, id, title: id } : null;
}

/** Full-text hits across both stores (`library_search`), ranked, scoped to
 *  the picked project. Debounced; a query shorter than two characters is
 *  answered by filtering the tree on titles instead. */
function useLibraryHits(query: string, workspace: string): { hits: LibraryHit[] | null; busy: boolean } {
  const [state, setState] = useState<{ key: string; hits: LibraryHit[] }>({ key: "", hits: [] });
  const q = query.trim();
  const key = `${workspace}\u0000${q}`;
  useEffect(() => {
    if (q.length < 2) return;
    let alive = true;
    const timer = window.setTimeout(() => {
      invoke<LibraryHit[]>("library_search", { query: q, workspace: workspace || null, kind: null, limit: 100 })
        .then((rows) => {
          if (alive) setState({ key, hits: rows ?? [] });
        })
        .catch(() => {
          if (alive) setState({ key, hits: [] });
        });
    }, 200);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [q, workspace, key]);
  if (q.length < 2) return { hits: null, busy: false };
  const current = state.key === key;
  return { hits: current ? state.hits : null, busy: !current };
}

/** Polling period of the backfill button while a backfill runs. Each status
 *  call syncs the index first, so not as tight as a batch's. */
const BACKFILL_POLL_MS = 5000;

/**
 * "补全摘要": the one-time summary backfill for items that have none. Shown
 * only while something is due or a run is going; asks with the count and the
 * estimated cost first, then shows done/planned and the cost so far, with a
 * stop (the next start resumes).
 */
function BackfillButton({ onFinished, onError }: { onFinished: () => void; onError: (e: string) => void }) {
  const { t } = useTranslation();
  const [status, setStatus] = useState<BackfillStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const wasRunning = useRef(false);

  const refresh = useCallback(async () => {
    try {
      const next = await invoke<BackfillStatus>("library_backfill_status");
      setStatus(next);
      // Summaries landed: titles, search and folder views read them.
      if (wasRunning.current && !next.running) onFinished();
      wasRunning.current = next.running;
    } catch {
      // A companion of the page; the tree works without it.
    }
  }, [onFinished]);
  useEffect(() => {
    void refresh();
  }, [refresh]);
  const running = !!status?.running;
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => void refresh(), BACKFILL_POLL_MS);
    return () => window.clearInterval(timer);
  }, [running, refresh]);

  const act = async (op: () => Promise<BackfillStatus>) => {
    setBusy(true);
    try {
      const next = await op();
      setStatus(next);
      wasRunning.current = next.running;
    } catch (e) {
      onError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (!status || (!running && status.due === 0)) return null;
  const run = status.run;
  if (running && run) {
    return (
      <span className={styles.backfill} data-backfill="running">
        <Spinner size={11} />
        <span>
          {t("library.backfill_running", "补全摘要 {{done}}/{{planned}} · ${{cost}}", {
            done: run.done,
            planned: run.planned,
            cost: run.costUsd.toFixed(2),
          })}
          {run.failed > 0 && ` · ${t("library.stat_failed", "失败 {{n}}", { n: run.failed })}`}
        </span>
        <button
          type="button"
          className={styles.upload_btn}
          onClick={() => void act(() => invoke<BackfillStatus>("library_backfill_stop"))}
          disabled={busy || run.stopRequested}
          title={t("library.backfill_stop_hint", "做完手上这几条就停；下次开始会接着跑")}
        >
          <Square size={11} strokeWidth={1.8} />
          {run.stopRequested ? t("library.backfill_stopping", "正在停止…") : t("library.backfill_stop", "停止")}
        </button>
      </span>
    );
  }
  const start = () => {
    const question = t(
      "library.backfill_confirm",
      "为 {{n}} 条还没有摘要的资料补写摘要？\n\n预计调用 AI {{n}} 次，约 ${{cost}}。3 路并发，可以随时停止，下次接着跑。",
      { n: status.due, cost: status.dueCostUsd.toFixed(2) },
    );
    if (window.confirm(question)) void act(() => invoke<BackfillStatus>("library_backfill_start"));
  };
  return (
    <button
      type="button"
      className={styles.upload_btn}
      onClick={start}
      disabled={busy}
      data-backfill="idle"
      title={t("library.backfill_hint", "{{n}} 条资料还没有摘要；摘要用于搜索和自动归档", { n: status.due })}
    >
      {busy ? <Spinner size={11} /> : <ScanText size={13} strokeWidth={1.6} />}
      {t("library.backfill", "补全摘要（{{n}} 条）", { n: status.due })}
    </button>
  );
}

/** What the preview column needs: a tree entry or a search hit. */
type SelectedItem = { kind: string; id: string; title: string };

/** The reader for the selected item: the side-rail pane of its kind, pinned
 *  open. Its close / collapse both clear the selection; there are no sibling
 *  cards here, so the "close others" family has nothing to act on. */
function LibraryDetail({ item, onClose }: { item: SelectedItem; onClose: () => void }) {
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
