// 资料 tab: the desktop 资料 page's structure, one level per screen — the
// project list, a project's overview (a card per folder), a folder's content
// (cards with summary, tags and thumbnails, filtered by type and ordered by
// time) and then the item itself. Each level is a history layer, so back (the
// button, the gesture, the hardware key) climbs one level. A query of two or
// more characters searches the index (both stores, bodies and AI summaries) in
// the current project, or everywhere at the top.
//
// Nothing here pulls the whole library: the project list and the overview come
// from an outline (counts plus the newest few items per folder), a folder's
// items are fetched when it opens and an item's record when it is tapped.
//
// The phone can file things: move an item to another folder, keep / rename /
// delete a folder an AI created, and upload — photographed receipts or picked
// files — into a project, where the host's AI files them in one batch whose
// summary can undo or move any item.

import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { ChevronDown, History, Library, Search, SearchX, Upload, WifiOff } from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { Artifact, SessionInfo, WikiDoc } from "../types";
import { useDraft } from "../draft";
import { useDelayedFlag } from "../useDelayedFlag";
import { HistoryLayer } from "../useNavStack";
import {
  fetchArtifactRecord,
  fetchLibraryOutline,
  fetchWikiDoc,
  findLibraryFolder,
  buildLibraryTree,
  isIngestRunning,
  libraryEntryKey,
  listIngestBatches,
  moveLibraryEntry,
  searchLibrary,
  uploadProjects,
  type IngestBatchSummary,
  type LibraryEntry,
  type LibraryFolderRec,
  type LibraryHit,
  type LibraryProjectTree,
  type UploadOutcome,
} from "../libraryApi";
import { ArtifactDetail } from "./ArtifactsView";
import { BatchRows, BatchView, BATCH_POLL_MS } from "./LibraryBatch";
import { FolderSheet } from "./LibraryTree";
import { FolderContent, ProjectOverview, ProjectRows, SearchCards, projectName } from "./LibraryBrowse";
import { UploadSheet } from "./LibraryUpload";
import { recentWorkspaceRows } from "./Composer";
import { AppHeader } from "./AppHeader";
import { EmptyState } from "./EmptyState";
import { SkeletonList, Spinner } from "./loading";
import { Presence } from "../Presence";
import tasksStyles from "./TasksView.module.css";
import styles from "./LibraryView.module.css";

interface Props {
  client: FleetTransport | null;
  /** This device's sessions: where it has been working, for the upload menu. */
  sessions: SessionInfo[];
  onOpenDoc: (doc: WikiDoc) => void;
}

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Debounced `library_search`; answers nothing under two characters. */
function useLibrarySearch(client: FleetTransport | null, query: string, workspace: string) {
  const [hits, setHits] = useState<LibraryHit[] | null>(null);
  const [searching, setSearching] = useState(false);
  const seq = useRef(0);
  useEffect(() => {
    const q = query.trim();
    if (!client || q.length < 2) {
      setHits(null);
      setSearching(false);
      return;
    }
    setSearching(true);
    const mine = ++seq.current;
    const timer = window.setTimeout(() => {
      searchLibrary(client, q, workspace || undefined)
        .then((h) => mine === seq.current && setHits(h ?? []))
        .catch(() => mine === seq.current && setHits([]))
        .finally(() => mine === seq.current && setSearching(false));
    }, 300);
    return () => window.clearTimeout(timer);
  }, [client, query, workspace]);
  return { hits, searching };
}

export function LibraryView({ client, sessions, onOpenDoc }: Props) {
  const [project, setProject] = useDraft<string>("library:project", "");
  /** The open folder of `project`; null = its overview, `""` = unfiled. */
  const [folder, setFolder] = useState<string | null>(null);
  const [uploadProject, setUploadProject] = useDraft<string>("library:uploadProject", "");
  const [query, setQuery] = useState("");

  // Every project, light: folders and counts, no items.
  const [projects, setProjects] = useState<LibraryProjectTree[] | null>(null);
  const [listError, setListError] = useState<string | null>(null);
  const loadProjects = useCallback(async () => {
    if (!client) return;
    try {
      setProjects(await fetchLibraryOutline(client, undefined, 0));
      setListError(null);
    } catch (e) {
      setListError(errText(e));
      setProjects((prev) => prev ?? []);
    }
  }, [client]);

  // The open project: its outline with the newest few items per folder.
  const [current, setCurrent] = useState<LibraryProjectTree | null>(null);
  const [currentError, setCurrentError] = useState<string | null>(null);
  const loadCurrent = useCallback(async () => {
    if (!client || !project) return;
    try {
      const got = await fetchLibraryOutline(client, project);
      setCurrent(got.find((p) => p.workspacePath === project) ?? got[0] ?? emptyProject(project));
      setCurrentError(null);
    } catch (e) {
      setCurrentError(errText(e));
    }
  }, [client, project]);
  /** Bumped to make an open folder fetch its items again. */
  const [reloadKey, setReloadKey] = useState(0);

  const [batches, setBatches] = useState<IngestBatchSummary[]>([]);
  const loadBatches = useCallback(async () => {
    if (!client) return;
    try {
      setBatches(await listIngestBatches(client, project || undefined));
    } catch {
      // The library is what matters; a batch list that failed just stays as it was.
    }
  }, [client, project]);

  useEffect(() => {
    void loadProjects();
  }, [loadProjects]);
  useEffect(() => {
    setCurrent(null);
    setFolder(null);
    void loadCurrent();
  }, [loadCurrent]);
  useEffect(() => {
    void loadBatches();
  }, [loadBatches]);
  const anyRunning = batches.some((b) => isIngestRunning(b.status));
  useEffect(() => {
    if (!anyRunning) return;
    const timer = window.setInterval(() => void loadBatches(), BATCH_POLL_MS);
    return () => window.clearInterval(timer);
  }, [anyRunning, loadBatches]);

  const refreshAll = useCallback(() => {
    void loadProjects();
    void loadCurrent();
    void loadBatches();
    setReloadKey((k) => k + 1);
  }, [loadProjects, loadCurrent, loadBatches]);
  // A batch that just finished filed things: every level has to show them.
  const wasRunning = useRef(false);
  useEffect(() => {
    if (wasRunning.current && !anyRunning) refreshAll();
    wasRunning.current = anyRunning;
  }, [anyRunning, refreshAll]);

  const { hits, searching } = useLibrarySearch(client, query, project);
  const showSearching = useDelayedFlag(query.trim().length >= 2 && searching);

  const uploadChoices = useMemo(
    () => uploadProjects(projects ?? [], recentWorkspaceRows(sessions, null)),
    [projects, sessions],
  );

  const [artifact, setArtifact] = useState<Artifact | null>(null);
  const [opening, setOpening] = useState<string | null>(null);
  const [openBatch, setOpenBatch] = useState<{ id: string; skipped?: UploadOutcome["skipped"] } | null>(null);
  const [batchesOpen, setBatchesOpen] = useState(false);
  const [folderSheet, setFolderSheet] = useState<LibraryFolderRec | null>(null);
  const [uploading, setUploading] = useState(false);
  const [moving, setMoving] = useState<Set<string>>(new Set());
  const [actionError, setActionError] = useState<string | null>(null);

  // An item's record is fetched when it is tapped, not listed up front.
  const openEntry = async (e: { kind: string; id: string }) => {
    if (!client || opening) return;
    const key = libraryEntryKey(e);
    setOpening(key);
    setActionError(null);
    try {
      if (e.kind === "doc") onOpenDoc(await fetchWikiDoc(client, e.id));
      else setArtifact(await fetchArtifactRecord(client, e.id));
    } catch (err) {
      setActionError(errText(err));
    } finally {
      setOpening(null);
    }
  };

  const moveEntry = async (e: LibraryEntry, to: string) => {
    if (!client || (e.folder ?? "") === to) return;
    const key = libraryEntryKey(e);
    setMoving((prev) => new Set(prev).add(key));
    setActionError(null);
    try {
      await moveLibraryEntry(client, e, to);
      await loadCurrent();
      setReloadKey((k) => k + 1);
    } catch (err) {
      setActionError(errText(err));
    } finally {
      setMoving((prev) => {
        const next = new Set(prev);
        next.delete(key);
        return next;
      });
    }
  };

  const openUpload = () => {
    if (project) setUploadProject(project);
    setUploading(true);
  };

  const searchMode = query.trim().length >= 2;
  const projectLabel =
    current?.workspaceName ||
    (projects ?? []).find((p) => p.workspacePath === project)?.workspaceName ||
    projectName(project);
  const folderNode = useMemo(
    () => (current && folder ? findLibraryFolder(buildLibraryTree(current).folders, folder) : null),
    [current, folder],
  );
  const running = batches.filter((b) => isIngestRunning(b.status));
  const error = listError || currentError || actionError;

  const uploadButton = (
    <button type="button" className={styles.uploadBtn} onClick={openUpload} disabled={!client} data-library-upload>
      <Upload size={15} strokeWidth={2} />
      {t("上传")}
    </button>
  );

  const searchBox = (
    <div className={tasksStyles.searchWrap}>
      <span className={tasksStyles.searchIcon}>
        <Search size={14} />
      </span>
      <input
        className={tasksStyles.search}
        type="search"
        placeholder={project ? t("在 {0} 里搜索…", projectLabel) : t("搜索标题 / 正文 / 摘要…")}
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />
      {showSearching && (
        <span className={tasksStyles.searchSpinner}>
          <Spinner size={12} label={t("搜索中…")} />
        </span>
      )}
    </div>
  );

  const batchEntry = batches.length > 0 && (
    <section className={styles.section}>
      {running.length > 0 && <BatchRows batches={running} onOpen={(id) => setOpenBatch({ id })} />}
      <button
        type="button"
        className={styles.batchToggle}
        onClick={() => setBatchesOpen((v) => !v)}
        aria-expanded={batchesOpen}
        data-library-batches
      >
        <History size={15} strokeWidth={1.6} />
        <span>{t("整理记录")}</span>
        <span className={styles.batchCount}>{batches.length}</span>
        <ChevronDown size={15} className={styles.batchCaret} data-open={batchesOpen} />
      </button>
      {batchesOpen && <BatchRows batches={batches} onOpen={(id) => setOpenBatch({ id })} />}
    </section>
  );

  let body: ReactNode;
  if (searchMode) {
    body =
      hits === null ? (
        <SkeletonList rows={4} />
      ) : hits.length === 0 ? (
        <EmptyState compact icon={SearchX} title={t("没有匹配的资料")} />
      ) : (
        <SearchCards client={client} hits={hits} hideProject={!!project} onOpen={(h) => void openEntry(h)} />
      );
  } else if (project && folder !== null) {
    body = current ? (
      <FolderContent
        client={client}
        project={current}
        path={folder}
        reloadKey={reloadKey}
        busy={moving}
        onOpenFolder={setFolder}
        onOpenEntry={(e) => void openEntry(e)}
        onMoveEntry={(e, f) => void moveEntry(e, f)}
      />
    ) : (
      <SkeletonList rows={5} avatar />
    );
  } else if (project) {
    body = current ? (
      <>
        <ProjectOverview
          project={current}
          onOpenFolder={setFolder}
          onOpenEntry={(e) => void openEntry(e)}
          onFolderMenu={setFolderSheet}
        />
        {batchEntry}
      </>
    ) : (
      <SkeletonList rows={6} />
    );
  } else if (projects === null) {
    body = client ? <SkeletonList rows={8} avatar /> : <EmptyState icon={WifiOff} title={t("桌面端离线")} />;
  } else if (projects.length === 0) {
    body = (
      <EmptyState
        icon={Library}
        title={t("还没有资料")}
        description={t("Agent 发布的文档、存进产出库的文件和你上传的资料会出现在这里。")}
      />
    );
  } else {
    body = (
      <>
        {batchEntry}
        <ProjectRows projects={projects} onOpen={setProject} />
      </>
    );
  }

  const leaveFolder = () => {
    if (!folder) return setFolder(null);
    // Climb one level: a subfolder returns to its parent folder.
    const up = folder.includes("/") ? folder.slice(0, folder.lastIndexOf("/")) : null;
    setFolder(up);
  };

  return (
    <div className={tasksStyles.wrapper} data-library-level={folder !== null ? "folder" : project ? "project" : "root"}>
      {project && <HistoryLayer key={`p:${project}`} onBack={() => setProject("")} />}
      {project && folder !== null && <HistoryLayer key={`f:${folder}`} onBack={leaveFolder} />}
      {project ? (
        <div className={styles.levelHead}>
          <AppHeader
            onBack={folder !== null ? leaveFolder : () => setProject("")}
            title={folder === null ? projectLabel : folder === "" ? t("未归档") : (folderNode?.label ?? folder)}
            titleAfter={
              folder !== null ? (
                <span className={styles.headCount}>
                  {folder === "" ? (current ? buildLibraryTree(current).unfiledTotal : "") : (folderNode?.total ?? "")}
                </span>
              ) : undefined
            }
            sub={folder !== null ? projectLabel : undefined}
            actions={uploadButton}
            seamless
          />
        </div>
      ) : (
        <div className={tasksStyles.pageHead}>
          <h1 className={tasksStyles.pageTitle}>{t("资料")}</h1>
          {uploadButton}
        </div>
      )}

      {searchBox}
      {error && <div className={styles.error}>{t("资料加载失败：{0}", error)}</div>}
      {opening && (
        <div className={styles.opening}>
          <Spinner size={12} /> {t("正在打开…")}
        </div>
      )}
      {body}

      <Presence when={!!artifact}>
        {artifact && <ArtifactDetail artifact={artifact} client={client} onBack={() => setArtifact(null)} />}
      </Presence>
      <Presence when={!!openBatch}>
        {openBatch && (
          <BatchView
            client={client}
            batchId={openBatch.id}
            skipped={openBatch.skipped}
            folders={
              (projects ?? []).find(
                (p) => p.workspacePath === (batches.find((b) => b.id === openBatch.id)?.workspacePath ?? project),
              )?.folders ??
              current?.folders ??
              []
            }
            onChanged={refreshAll}
            onOpenArtifact={(id) => void openEntry({ kind: "artifact", id })}
            onBack={() => {
              setOpenBatch(null);
              void loadBatches();
            }}
          />
        )}
      </Presence>
      <Presence when={!!folderSheet}>
        {folderSheet && (
          <FolderSheet
            client={client}
            folder={folderSheet}
            onClose={() => setFolderSheet(null)}
            onChanged={() => {
              void loadCurrent();
              void loadProjects();
            }}
          />
        )}
      </Presence>
      <Presence when={uploading}>
        {uploading && (
          <UploadSheet
            client={client}
            projects={uploadChoices}
            project={uploadProject}
            onProject={setUploadProject}
            onClose={() => setUploading(false)}
            onUploaded={(outcome) => {
              setUploading(false);
              setProject(outcome.batch.workspacePath);
              setOpenBatch({ id: outcome.batch.id, skipped: outcome.skipped });
              void loadBatches();
            }}
          />
        )}
      </Presence>
    </div>
  );
}

function emptyProject(workspacePath: string): LibraryProjectTree {
  return { workspacePath, workspaceName: projectName(workspacePath), folders: [], items: [], counts: {} };
}
