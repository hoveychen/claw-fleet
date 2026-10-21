// The phone's 资料 browsing levels — the desktop 资料 page's structure laid out
// one level per screen: the project list, a project's overview (a card per
// folder with its description, count, subfolders and newest items), a folder's
// content (cards with summary, tags, type, subfolder, time and a thumbnail,
// filtered by type and ordered by time) and search hits drawn as the same
// cards. The listing logic is shared with the desktop (shared-ts/libraryBrowse.ts).
//
// The project list and overview come from an outline (`library_tree` with
// `outline: true`); a folder's items, summaries included, are fetched when it is
// opened, so the phone never pulls the whole tree.

import { useEffect, useMemo, useRef, useState } from "react";
import {
  ChevronRight,
  FileText,
  Film,
  Folder,
  FolderInput,
  FolderOpen,
  Inbox,
  MoreHorizontal,
  Sparkles,
} from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import {
  buildLibraryTree,
  fetchFolderItems,
  fetchLibraryThumb,
  filterListing,
  findLibraryFolder,
  folderAwaitingReview,
  folderCards,
  folderChoices,
  libraryEntryKey,
  libraryTypeGroup,
  listingOf,
  orderLibraryProjects,
  projectTotals,
  snippetRuns,
  typeCounts,
  visibleTypeGroups,
  type LibraryEntry,
  type LibraryFolderNode,
  type LibraryFolderRec,
  type LibraryHit,
  type LibraryProjectTree,
  type LibrarySort,
  type LibraryTypeGroup,
} from "../libraryApi";
import { repoRootPath } from "../../../shared-ts/repoPath";
import { KIND_ICON } from "./ArtifactsView";
import { timeAgo } from "./TasksView";
import { awaitingFolders } from "./LibraryTree";
import { EmptyState } from "./EmptyState";
import { SkeletonList, Spinner } from "./loading";
import styles from "./LibraryBrowse.module.css";

export const TYPE_LABEL: Record<LibraryTypeGroup, string> = {
  doc: "文档",
  artifact: "产出",
  text: "文本",
  image: "图片",
  video: "视频",
  sheet: "表格",
  slides: "幻灯片",
  audio: "音频",
  other: "其他",
};

function errText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

// ── Project list ────────────────────────────────────────────────────────────

export function ProjectRows({
  projects,
  onOpen,
}: {
  projects: LibraryProjectTree[];
  onOpen: (workspacePath: string) => void;
}) {
  const ordered = useMemo(() => orderLibraryProjects(projects), [projects]);
  return (
    <div className={styles.rows} data-library-projects>
      {ordered.map((p) => {
        const review = awaitingFolders(p).length;
        const { total, unfiled } = projectTotals(p);
        return (
          <button
            key={p.workspacePath}
            type="button"
            className={styles.row}
            onClick={() => onOpen(p.workspacePath)}
            data-project={p.workspaceName}
          >
            <FolderOpen size={18} strokeWidth={1.5} className={styles.rowIcon} />
            <span className={styles.rowBody}>
              <span className={styles.rowTitleLine}>
                <span className={styles.rowTitle}>{p.workspaceName}</span>
                {review > 0 && (
                  <span className={styles.aiBadge}>
                    <Sparkles size={10} strokeWidth={1.8} />
                    {t("{0} 个新目录待确认", review)}
                  </span>
                )}
              </span>
              <span className={styles.rowMeta}>
                {[
                  t("{0} 份资料", total),
                  t("{0} 个目录", p.folders.length),
                  unfiled > 0 ? t("未归档 {0}", unfiled) : "",
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            </span>
            <ChevronRight size={16} className={styles.chevron} />
          </button>
        );
      })}
    </div>
  );
}

// ── Project overview ────────────────────────────────────────────────────────

export function ProjectOverview({
  project,
  onOpenFolder,
  onOpenEntry,
  onFolderMenu,
}: {
  project: LibraryProjectTree;
  /** `""` opens the unfiled bucket. */
  onOpenFolder: (path: string) => void;
  onOpenEntry: (e: LibraryEntry) => void;
  onFolderMenu: (f: LibraryFolderRec) => void;
}) {
  const view = useMemo(() => buildLibraryTree(project), [project]);
  const cards = useMemo(() => folderCards(view), [view]);
  const byPath = useMemo(() => new Map(project.folders.map((f) => [f.path, f])), [project.folders]);
  const review = awaitingFolders(project).length;
  const { total } = projectTotals(project);

  return (
    <div className={styles.level} data-library-overview>
      <div className={styles.levelMeta}>
        {[
          t("{0} 份资料", total),
          t("{0} 个目录", project.folders.length),
          view.unfiledTotal > 0 ? t("未归档 {0}", view.unfiledTotal) : "",
        ]
          .filter(Boolean)
          .join(" · ")}
      </div>
      {review > 0 && (
        <div className={styles.reviewBanner}>
          {t("AI 新建了 {0} 个目录，点目录右边的 ⋯ 可保留、改名或删除。", review)}
        </div>
      )}
      {cards.length === 0 && view.unfiledTotal === 0 && (
        <EmptyState
          compact
          icon={FolderOpen}
          title={t("这个项目还没有资料")}
          description={t("拍张发票或选个文件上传，AI 会帮你归档。")}
        />
      )}
      {cards.map(({ node, recent }) => (
        <FolderCard
          key={node.path}
          node={node}
          recent={recent}
          rec={byPath.get(node.path)}
          onOpen={() => onOpenFolder(node.path)}
          onOpenChild={onOpenFolder}
          onOpenEntry={onOpenEntry}
          onMenu={onFolderMenu}
        />
      ))}
      {view.unfiledTotal > 0 && (
        <FolderCard
          node={{
            path: "",
            label: t("未归档"),
            description: t("还没放进任何目录的资料"),
            origin: null,
            children: [],
            items: view.unfiled,
            total: view.unfiledTotal,
          }}
          recent={view.unfiled.slice(0, 3)}
          unfiled
          onOpen={() => onOpenFolder("")}
          onOpenChild={onOpenFolder}
          onOpenEntry={onOpenEntry}
        />
      )}
    </div>
  );
}

function FolderCard({
  node,
  recent,
  rec,
  unfiled,
  onOpen,
  onOpenChild,
  onOpenEntry,
  onMenu,
}: {
  node: LibraryFolderNode;
  recent: LibraryEntry[];
  rec?: LibraryFolderRec;
  unfiled?: boolean;
  onOpen: () => void;
  onOpenChild: (path: string) => void;
  onOpenEntry: (e: LibraryEntry) => void;
  onMenu?: (f: LibraryFolderRec) => void;
}) {
  const review = folderAwaitingReview(node.origin);
  const Icon = unfiled ? Inbox : Folder;
  return (
    <section className={styles.card} data-folder-card={node.path}>
      <div className={styles.cardHead}>
        <button type="button" className={styles.cardOpen} onClick={onOpen} data-folder={node.path}>
          <Icon size={17} strokeWidth={1.6} className={styles.folderIcon} />
          <span className={styles.cardTitle}>{node.label}</span>
          {review && (
            <span className={styles.aiBadge} title={node.origin?.reason}>
              <Sparkles size={10} strokeWidth={1.8} />
              {t("AI 新建")}
            </span>
          )}
          <span className={styles.count}>{node.total}</span>
          <ChevronRight size={15} className={styles.chevron} />
        </button>
        {rec && onMenu && (
          <button type="button" className={styles.more} onClick={() => onMenu(rec)} aria-label={t("目录操作")}>
            <MoreHorizontal size={16} />
          </button>
        )}
      </div>
      {node.description && <div className={styles.cardDesc}>{node.description}</div>}
      {node.children.length > 0 && (
        <SubfolderChips nodes={node.children} onOpen={onOpenChild} />
      )}
      {recent.length > 0 ? (
        <div className={styles.recent}>
          {recent.map((e) => (
            <button key={libraryEntryKey(e)} type="button" className={styles.recentRow} onClick={() => onOpenEntry(e)}>
              <EntryIcon entry={e} size={13} />
              <span className={styles.recentTitle}>{e.title}</span>
              <span className={styles.recentTime}>{timeAgo(e.updatedMs)}</span>
            </button>
          ))}
        </div>
      ) : (
        node.total === 0 && <div className={styles.cardEmpty}>{t("空目录")}</div>
      )}
    </section>
  );
}

function SubfolderChips({ nodes, onOpen }: { nodes: LibraryFolderNode[]; onOpen: (path: string) => void }) {
  return (
    <div className={styles.subChips}>
      {nodes.map((c) => (
        <button key={c.path} type="button" className={styles.subChip} onClick={() => onOpen(c.path)} data-subfolder={c.path}>
          <Folder size={12} strokeWidth={1.6} />
          {c.label}
          <span className={styles.subCount}>{c.total}</span>
        </button>
      ))}
    </div>
  );
}

// ── Folder content ──────────────────────────────────────────────────────────

export function FolderContent({
  client,
  project,
  path,
  reloadKey,
  busy,
  onOpenFolder,
  onOpenEntry,
  onMoveEntry,
}: {
  client: FleetTransport | null;
  project: LibraryProjectTree;
  /** `""` = the unfiled bucket. */
  path: string;
  /** Bumped by the parent after a move or a finished upload: fetch again. */
  reloadKey: number;
  busy: ReadonlySet<string>;
  onOpenFolder: (path: string) => void;
  onOpenEntry: (e: LibraryEntry) => void;
  onMoveEntry: (e: LibraryEntry, folder: string) => void;
}) {
  const view = useMemo(() => buildLibraryTree(project), [project]);
  const node = path ? findLibraryFolder(view.folders, path) : null;
  const [items, setItems] = useState<LibraryEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [group, setGroup] = useState<LibraryTypeGroup | null>(null);
  const [sort, setSort] = useState<LibrarySort>("newest");

  useEffect(() => {
    setGroup(null);
  }, [path]);
  useEffect(() => {
    if (!client) return;
    let alive = true;
    setError(null);
    fetchFolderItems(client, project.workspacePath, path)
      .then((got) => alive && setItems(got ?? []))
      .catch((e) => {
        if (!alive) return;
        setError(errText(e));
        setItems((prev) => prev ?? []);
      });
    return () => {
      alive = false;
    };
  }, [client, project.workspacePath, path, reloadKey]);
  // A different folder starts from a skeleton, not the last one's cards.
  useEffect(() => setItems(null), [path]);

  const rows = useMemo(() => (items ? listingOf(items, path) : []), [items, path]);
  const counts = useMemo(() => typeCounts(rows), [rows]);
  const shown = useMemo(() => filterListing(rows, group, sort), [rows, group, sort]);
  const choices = useMemo(() => folderChoices(project.folders), [project.folders]);
  const groups = visibleTypeGroups(counts);

  return (
    <div className={styles.level} data-library-folder={path}>
      {node?.description && <div className={styles.folderDesc}>{node.description}</div>}
      {node && node.children.length > 0 && <SubfolderChips nodes={node.children} onOpen={onOpenFolder} />}
      {items && rows.length > 0 && (
        <div className={styles.filterBar}>
          <div className={styles.typeChips} role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={group === null}
              className={styles.typeChip}
              data-active={group === null}
              onClick={() => setGroup(null)}
            >
              {t("全部")} <span className={styles.chipCount}>{rows.length}</span>
            </button>
            {groups.map((g) => (
              <button
                key={g}
                type="button"
                role="tab"
                aria-selected={group === g}
                className={styles.typeChip}
                data-active={group === g}
                onClick={() => setGroup(g)}
              >
                {t(TYPE_LABEL[g])} <span className={styles.chipCount}>{counts[g]}</span>
              </button>
            ))}
          </div>
          <select
            className={styles.sort}
            value={sort}
            onChange={(e) => setSort(e.target.value as LibrarySort)}
            aria-label={t("排序")}
          >
            <option value="newest">{t("最新在前")}</option>
            <option value="oldest">{t("最早在前")}</option>
          </select>
        </div>
      )}
      {error && <div className={styles.error}>{t("资料加载失败：{0}", error)}</div>}
      {items === null ? (
        <SkeletonList rows={5} avatar />
      ) : rows.length === 0 ? (
        <EmptyState compact icon={FolderOpen} title={t("空目录")} />
      ) : (
        <div className={styles.cards}>
          {shown.map((r) => (
            <EntryCard
              key={libraryEntryKey(r.entry)}
              client={client}
              entry={r.entry}
              where={r.sub}
              onOpen={() => onOpenEntry(r.entry)}
              move={{ choices, busy: busy.has(libraryEntryKey(r.entry)), onMove: (f) => onMoveEntry(r.entry, f) }}
            />
          ))}
        </div>
      )}
    </div>
  );
}

// ── Cards ───────────────────────────────────────────────────────────────────

function EntryIcon({ entry, size }: { entry: Pick<LibraryEntry, "kind" | "artifactKind">; size: number }) {
  const Icon = entry.kind === "doc" ? FileText : (KIND_ICON[entry.artifactKind] ?? FileText);
  return <Icon size={size} strokeWidth={1.5} className={styles.entryIcon} />;
}

/** Thumbnails fetched this session, by artifact id (null = none to show). */
const thumbCache = new Map<string, string | null>();
/** At most this many `library_thumb` requests in flight; the rest queue. */
const THUMB_PARALLEL = 3;
let thumbsInFlight = 0;
const thumbQueue: Array<() => void> = [];

function queueThumb(run: () => Promise<void>) {
  const start = () => {
    thumbsInFlight += 1;
    void run().finally(() => {
      thumbsInFlight -= 1;
      thumbQueue.shift()?.();
    });
  };
  if (thumbsInFlight < THUMB_PARALLEL) start();
  else thumbQueue.push(start);
}

/** An image's thumbnail once its card scrolls into view; null until then, and
 *  for anything that is not an image. */
function useThumb(client: FleetTransport | null, id: string, isImage: boolean) {
  const ref = useRef<HTMLSpanElement | null>(null);
  const [url, setUrl] = useState<string | null>(() => thumbCache.get(id) ?? null);
  useEffect(() => {
    if (!client || !isImage || thumbCache.has(id)) return;
    const el = ref.current;
    if (!el) return;
    let alive = true;
    const io = new IntersectionObserver(
      (hits) => {
        if (!hits.some((h) => h.isIntersecting)) return;
        io.disconnect();
        queueThumb(async () => {
          if (thumbCache.has(id)) {
            if (alive) setUrl(thumbCache.get(id) ?? null);
            return;
          }
          const got = await fetchLibraryThumb(client, id).catch(() => null);
          thumbCache.set(id, got);
          if (alive) setUrl(got);
        });
      },
      { rootMargin: "200px" },
    );
    io.observe(el);
    return () => {
      alive = false;
      io.disconnect();
    };
  }, [client, id, isImage]);
  return { ref, url };
}

export function EntryCard({
  client,
  entry,
  where,
  project,
  snippet,
  onOpen,
  move,
}: {
  client: FleetTransport | null;
  entry: LibraryEntry;
  /** The subfolder (or, for a search hit, the folder) the item sits in. */
  where?: string;
  /** A project name to show — search hits across projects. */
  project?: string;
  /** A search hit's `<mark>` excerpt, drawn in place of the summary. */
  snippet?: string;
  onOpen: () => void;
  move?: { choices: Array<{ path: string; label: string }>; busy: boolean; onMove: (folder: string) => void };
}) {
  const group = libraryTypeGroup(entry);
  const thumb = useThumb(client, entry.id, entry.kind === "artifact" && entry.artifactKind === "image");
  const runs = snippet ? snippetRuns(snippet) : [];
  const tags = entry.tags ?? [];
  return (
    <div className={styles.entry} data-entry={entry.id}>
      <button type="button" className={styles.entryMain} onClick={onOpen}>
        <span className={styles.thumb} ref={thumb.ref} data-group={group}>
          {thumb.url ? (
            <img src={thumb.url} alt="" className={styles.thumbImg} />
          ) : entry.artifactKind === "video" ? (
            <Film size={20} strokeWidth={1.4} />
          ) : (
            <EntryIcon entry={entry} size={20} />
          )}
        </span>
        <span className={styles.entryBody}>
          <span className={styles.entryTitle}>{entry.title}</span>
          {runs.length > 0 ? (
            <span className={styles.entrySummary}>
              {runs.map((r, i) => (r.mark ? <mark key={i} className={styles.mark}>{r.text}</mark> : <span key={i}>{r.text}</span>))}
            </span>
          ) : (
            entry.summary && <span className={styles.entrySummary}>{entry.summary}</span>
          )}
          {tags.length > 0 && (
            <span className={styles.tags}>
              {tags.slice(0, 5).map((tag) => (
                <span key={tag} className={styles.tag}>
                  {tag}
                </span>
              ))}
            </span>
          )}
          <span className={styles.entryMeta}>
            <span>{t(TYPE_LABEL[group])}</span>
            {project && <span className={styles.metaProject}>{project}</span>}
            {where && (
              <span className={styles.metaWhere}>
                <Folder size={11} strokeWidth={1.6} />
                {where}
              </span>
            )}
            <span className={styles.metaTime}>{timeAgo(entry.updatedMs)}</span>
          </span>
        </span>
      </button>
      {move && (
        // A native select under the icon: tapping it opens the system picker
        // (the wheel on iOS) — no custom sheet to dismiss.
        <label className={styles.moveBtn} aria-label={t("移到…")}>
          {move.busy ? <Spinner size={14} /> : <FolderInput size={16} strokeWidth={1.6} />}
          <select
            className={styles.moveSelect}
            value={entry.folder ?? ""}
            disabled={move.busy}
            onChange={(e) => move.onMove(e.target.value)}
          >
            <option value="">{t("未归档")}</option>
            {move.choices.map((c) => (
              <option key={c.path} value={c.path}>
                {c.label}
              </option>
            ))}
          </select>
        </label>
      )}
    </div>
  );
}

/** A search hit as an entry card, labelled with the folder it is filed in. */
export function SearchCards({
  client,
  hits,
  hideProject,
  onOpen,
}: {
  client: FleetTransport | null;
  hits: LibraryHit[];
  hideProject: boolean;
  onOpen: (h: LibraryHit) => void;
}) {
  return (
    <div className={styles.cards} data-library-hits>
      {hits.map((h) => (
        <EntryCard
          key={`${h.kind}:${h.id}`}
          client={client}
          entry={hitEntry(h)}
          where={h.folder || t("未归档")}
          project={hideProject ? undefined : projectName(h.project)}
          snippet={h.snippet || undefined}
          onOpen={() => onOpen(h)}
        />
      ))}
    </div>
  );
}

/** A flat run of entry cards (a task's 资料), each labelled with its folder,
 *  and with its project when that is not `homeProject`. */
export function EntryCards({
  client,
  entries,
  homeProject,
  onOpen,
}: {
  client: FleetTransport | null;
  entries: LibraryEntry[];
  homeProject: string;
  onOpen: (e: LibraryEntry) => void;
}) {
  return (
    <div className={styles.cards}>
      {entries.map((e) => (
        <EntryCard
          key={libraryEntryKey(e)}
          client={client}
          entry={e}
          where={e.folder || t("未归档")}
          project={repoRootPath(e.workspacePath) === homeProject ? undefined : projectName(e.workspacePath)}
          onOpen={() => onOpen(e)}
        />
      ))}
    </div>
  );
}

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
    summary: h.summary,
    tags: h.tags,
  };
}

export function projectName(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).pop() ?? path;
}

