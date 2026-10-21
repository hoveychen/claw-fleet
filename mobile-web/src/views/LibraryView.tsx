// 资料 tab: the library as one folder tree per project, the same tree the
// desktop 资料 page draws. Without a project chosen it lists the projects;
// with one it shows that project's folders, its unfiled bucket and its recent
// uploads. A query of two or more characters searches the index (both stores,
// bodies and AI summaries) instead.
//
// The phone can file things: move an item to another folder, keep / rename /
// delete a folder an AI created, and upload — photographed receipts or picked
// files — into a project, where the host's AI files them in one batch whose
// summary can undo or move any item.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { ChevronDown, Library, Search, SearchX, Upload, WifiOff, X } from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { Artifact, SessionInfo, WikiDoc } from "../types";
import { useDraft } from "../draft";
import { useDelayedFlag } from "../useDelayedFlag";
import {
  fetchLibraryTree,
  isIngestRunning,
  libraryEntryKey,
  listIngestBatches,
  moveLibraryEntry,
  searchLibrary,
  snippetRuns,
  uploadProjects,
  type IngestBatchSummary,
  type LibraryEntry,
  type LibraryFolderRec,
  type LibraryHit,
  type LibraryProjectTree,
  type UploadOutcome,
} from "../libraryApi";
import { useLibrary } from "./LibraryList";
import { ArtifactDetail } from "./ArtifactsView";
import { BatchRows, BatchView, BATCH_POLL_MS } from "./LibraryBatch";
import { FolderSheet, ProjectList, ProjectTree, awaitingFolders, type EntryFilter } from "./LibraryTree";
import { UploadSheet } from "./LibraryUpload";
import { recentWorkspaceRows } from "./Composer";
import { EmptyState } from "./EmptyState";
import { SkeletonList, Spinner } from "./loading";
import { Presence } from "../Presence";
import tasksStyles from "./TasksView.module.css";
import listStyles from "./LibraryList.module.css";
import styles from "./LibraryView.module.css";

const TYPES: Array<[EntryFilter, string]> = [
  ["all", "全部"],
  ["doc", "文档"],
  ["artifact", "产出"],
];

/** Recent uploads shown above a tree (or above the project list). */
const BATCHES_SHOWN = 4;

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
  const lib = useLibrary(client);
  const refreshLib = lib.refresh;
  const [type, setType] = useDraft<EntryFilter>("library:type", "all");
  const [project, setProject] = useDraft<string>("library:project", "");
  const [uploadProject, setUploadProject] = useDraft<string>("library:uploadProject", "");
  const [query, setQuery] = useState("");

  const [tree, setTree] = useState<LibraryProjectTree[] | null>(null);
  const [treeError, setTreeError] = useState<string | null>(null);
  const loadTree = useCallback(async () => {
    if (!client) return;
    try {
      setTree(await fetchLibraryTree(client));
      setTreeError(null);
    } catch (e) {
      setTreeError(errText(e));
      setTree((prev) => prev ?? []);
    }
  }, [client]);

  const [batches, setBatches] = useState<IngestBatchSummary[]>([]);
  const loadBatches = useCallback(async () => {
    if (!client) return;
    try {
      setBatches(await listIngestBatches(client, project || undefined));
    } catch {
      // The tree is what matters; a batch list that failed just stays as it was.
    }
  }, [client, project]);

  useEffect(() => {
    void loadTree();
  }, [loadTree]);
  useEffect(() => {
    void loadBatches();
  }, [loadBatches]);
  const anyRunning = batches.some((b) => isIngestRunning(b.status));
  useEffect(() => {
    if (!anyRunning) return;
    const timer = window.setInterval(() => void loadBatches(), BATCH_POLL_MS);
    return () => window.clearInterval(timer);
  }, [anyRunning, loadBatches]);
  // A batch that just finished filed things: the tree has to show them.
  const wasRunning = useRef(false);
  useEffect(() => {
    if (wasRunning.current && !anyRunning) {
      void loadTree();
      void refreshLib();
    }
    wasRunning.current = anyRunning;
  }, [anyRunning, loadTree, refreshLib]);

  const refreshAll = useCallback(() => {
    void loadTree();
    void loadBatches();
    void refreshLib();
  }, [loadTree, loadBatches, refreshLib]);

  const { hits, searching } = useLibrarySearch(client, query, project);
  const showSearching = useDelayedFlag(query.trim().length >= 2 && searching);

  const docs = useMemo(() => {
    const m = new Map<string, WikiDoc>();
    for (const it of lib.items ?? []) if (it.type === "doc") m.set(it.doc.slug, it.doc);
    return m;
  }, [lib.items]);
  const artifacts = useMemo(() => {
    const m = new Map<string, Artifact>();
    for (const it of lib.items ?? []) if (it.type === "artifact") m.set(it.artifact.id, it.artifact);
    return m;
  }, [lib.items]);

  const current = useMemo(
    () => (project ? (tree ?? []).find((p) => p.workspacePath === project) ?? null : null),
    [tree, project],
  );
  const projectFolders = useMemo(() => {
    const m = new Map<string, LibraryFolderRec[]>();
    for (const p of tree ?? []) m.set(p.workspacePath, p.folders);
    return m;
  }, [tree]);

  const uploadChoices = useMemo(
    () => uploadProjects(tree ?? [], recentWorkspaceRows(sessions, null)),
    [tree, sessions],
  );

  const [openArtifact, setOpenArtifact] = useState<string | null>(null);
  const [openBatch, setOpenBatch] = useState<{ id: string; skipped?: UploadOutcome["skipped"] } | null>(null);
  const [folderSheet, setFolderSheet] = useState<LibraryFolderRec | null>(null);
  const [uploading, setUploading] = useState(false);
  const [moving, setMoving] = useState<Set<string>>(new Set());
  const [actionError, setActionError] = useState<string | null>(null);

  const openEntry = (e: { kind: string; id: string }) => {
    if (e.kind === "doc") {
      const doc = docs.get(e.id);
      if (doc) onOpenDoc(doc);
      else void lib.refresh();
    } else {
      setOpenArtifact(e.id);
    }
  };

  const moveEntry = async (e: LibraryEntry, folder: string) => {
    if (!client || (e.folder ?? "") === folder) return;
    const key = libraryEntryKey(e);
    setMoving((prev) => new Set(prev).add(key));
    setActionError(null);
    try {
      await moveLibraryEntry(client, e, folder);
      await loadTree();
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

  const artifact = openArtifact ? artifacts.get(openArtifact) ?? null : null;
  const searchMode = query.trim().length >= 2;
  const recent = batches.slice(0, BATCHES_SHOWN);
  const review = current ? awaitingFolders(current).length : 0;

  return (
    <div className={tasksStyles.wrapper}>
      <div className={tasksStyles.pageHead}>
        <h1 className={tasksStyles.pageTitle}>{t("资料")}</h1>
        <label className={tasksStyles.scopeSelectWrap} data-on={!!project}>
          <select
            className={tasksStyles.scopeSelect}
            value={project}
            onChange={(e) => setProject(e.target.value)}
            aria-label={t("全部项目")}
          >
            <option value="">{t("全部项目")}</option>
            {(tree ?? []).map((p) => (
              <option key={p.workspacePath} value={p.workspacePath}>
                {p.workspaceName}
              </option>
            ))}
            {project && !(tree ?? []).some((p) => p.workspacePath === project) && (
              <option value={project}>{project.split(/[\\/]/).filter(Boolean).pop() ?? project}</option>
            )}
          </select>
          <ChevronDown size={14} aria-hidden="true" />
        </label>
        {project && (
          <button
            className={tasksStyles.scopeClear}
            onClick={() => setProject("")}
            aria-label={t("显示全部项目")}
          >
            <X size={14} strokeWidth={2.2} />
          </button>
        )}
        <button
          type="button"
          className={styles.uploadBtn}
          onClick={openUpload}
          disabled={!client}
          data-library-upload
        >
          <Upload size={15} strokeWidth={2} />
          {t("上传")}
        </button>
      </div>

      <div className={tasksStyles.searchWrap}>
        <span className={tasksStyles.searchIcon}>
          <Search size={14} />
        </span>
        <input
          className={tasksStyles.search}
          type="search"
          placeholder={t("搜索标题 / 正文 / 摘要…")}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
        />
        {showSearching && (
          <span className={tasksStyles.searchSpinner}>
            <Spinner size={12} label={t("搜索中…")} />
          </span>
        )}
      </div>

      {!searchMode && project && (
        <div className={styles.types} role="tablist">
          {TYPES.map(([value, label]) => (
            <button
              key={value}
              type="button"
              role="tab"
              aria-selected={type === value}
              className={styles.type}
              data-active={type === value}
              onClick={() => setType(value)}
            >
              {t(label)}
            </button>
          ))}
        </div>
      )}

      {(treeError || lib.error) && (
        <div className={styles.error}>{t("资料加载失败：{0}", treeError || lib.error || "")}</div>
      )}
      {actionError && <div className={styles.error}>{actionError}</div>}

      {searchMode ? (
        hits === null ? (
          <SkeletonList rows={4} />
        ) : hits.length === 0 ? (
          <EmptyState compact icon={SearchX} title={t("没有匹配的资料")} />
        ) : (
          <SearchHits hits={hits} hideProject={!!project} onOpen={openEntry} />
        )
      ) : tree === null ? (
        client ? (
          <SkeletonList rows={8} avatar />
        ) : (
          <EmptyState icon={WifiOff} title={t("桌面端离线")} />
        )
      ) : (
        <>
          {recent.length > 0 && (
            <section className={styles.section}>
              <h2 className={styles.sectionHead}>{t("最近上传")}</h2>
              <BatchRows batches={recent} onOpen={(id) => setOpenBatch({ id })} />
            </section>
          )}
          {current ? (
            <section className={styles.section}>
              {review > 0 && (
                <div className={styles.reviewBanner}>
                  {t("AI 新建了 {0} 个目录，点目录右边的 ⋯ 可保留、改名或删除。", review)}
                </div>
              )}
              <ProjectTree
                key={current.workspacePath}
                project={current}
                filter={type}
                docs={docs}
                artifacts={artifacts}
                busy={moving}
                onOpenEntry={openEntry}
                onMoveEntry={(e, f) => void moveEntry(e, f)}
                onFolderMenu={setFolderSheet}
              />
            </section>
          ) : project ? (
            <EmptyState
              compact
              icon={Library}
              title={t("这个项目还没有资料")}
              description={t("拍张发票或选个文件上传，AI 会帮你归档。")}
            />
          ) : tree.length === 0 ? (
            <EmptyState
              icon={Library}
              title={t("还没有资料")}
              description={t("Agent 发布的文档、存进产出库的文件和你上传的资料会出现在这里。")}
            />
          ) : (
            <ProjectList projects={tree} onOpen={setProject} />
          )}
        </>
      )}

      <Presence when={!!artifact}>
        {artifact && (
          <ArtifactDetail artifact={artifact} client={client} onBack={() => setOpenArtifact(null)} />
        )}
      </Presence>
      <Presence when={!!openBatch}>
        {openBatch && (
          <BatchView
            client={client}
            batchId={openBatch.id}
            skipped={openBatch.skipped}
            folders={
              projectFolders.get(batches.find((b) => b.id === openBatch.id)?.workspacePath ?? project) ??
              current?.folders ??
              []
            }
            onChanged={refreshAll}
            onOpenArtifact={(id) => setOpenArtifact(id)}
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
            onChanged={() => void loadTree()}
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

function SearchHits({
  hits,
  hideProject,
  onOpen,
}: {
  hits: LibraryHit[];
  hideProject: boolean;
  onOpen: (h: LibraryHit) => void;
}) {
  return (
    <div className={listStyles.list} data-library-hits>
      {hits.map((h) => {
        const runs = snippetRuns(h.snippet || h.summary);
        const where = h.folder || t("未归档");
        return (
          <button key={`${h.kind}:${h.id}`} type="button" className={listStyles.row} onClick={() => onOpen(h)}>
            <span className={listStyles.body}>
              <span className={listStyles.line1}>
                <span className={listStyles.title}>{h.title}</span>
                {!hideProject && h.project && <span className={listStyles.project}>{projectName(h.project)}</span>}
              </span>
              <span className={listStyles.line2}>
                {[h.kind === "doc" ? t("文档") : t("产出"), where, h.field === "summary" ? t("摘要命中") : ""]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
              {runs.length > 0 && (
                <span className={listStyles.snippet}>
                  {runs.map((r, i) => (r.mark ? <mark key={i} className={styles.mark}>{r.text}</mark> : <span key={i}>{r.text}</span>))}
                </span>
              )}
              {h.tags.length > 0 && (
                <span className={styles.hitTags}>
                  {h.tags.map((tag) => (
                    <span key={tag} className={styles.hitTag}>
                      {tag}
                    </span>
                  ))}
                </span>
              )}
            </span>
          </button>
        );
      })}
    </div>
  );
}

function projectName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}
