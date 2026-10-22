import { useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";
import type { TFunction } from "i18next";
import {
  Archive,
  BookOpen,
  Check,
  ChevronRight,
  Download,
  Film,
  FileSpreadsheet,
  FileText,
  FileType,
  FileUp,
  Folder,
  FolderSync,
  FolderUp,
  Image as ImageIcon,
  Inbox,
  ListChecks,
  Music,
  Presentation,
  Sparkles,
  X,
} from "lucide-react";

import {
  filterListing,
  findLibraryFolder,
  folderCards,
  foldersAwaitingReview,
  LIBRARY_SOURCE_GROUPS,
  listingOf,
  projectTotals,
  typeCounts,
  visibleTypeGroups,
  type FolderListing,
  type LibrarySort,
  type LibraryTypeGroup,
} from "../../../shared-ts/libraryBrowse";
import {
  libraryEntryKey,
  libraryEntryMatches,
  type LibraryEntry,
  type LibraryFolderNode,
  type LibraryProjectTree,
  type LibraryTreeView,
} from "../../../shared-ts/libraryTree";
import { isIngestRunning, ingestBatchKey, type IngestBatchSummary } from "../../../shared-ts/libraryIngest";
import { snippetRuns, type LibraryHit } from "../../../shared-ts/librarySearch";
import { artifactBlobUrl, artifactPosterUrl } from "../artifactAssets";
import { thumbMode } from "../officePreview";
import type { PickKind } from "../libraryUpload";
import ArtifactThumb from "./ArtifactThumb";
import { BatchRow } from "./LibraryBatch";
import { originTitle } from "./LibraryFolderRail";
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

function iconOf(e: { kind: string; artifactKind: string }) {
  return e.kind === "doc" ? BookOpen : (ARTIFACT_ICON[e.artifactKind] ?? FileText);
}

/** Literal `t()` calls so the locale-key guard sees every key. */
function kindLabel(t: TFunction, e: { kind: string; artifactKind: string }): string {
  if (e.kind === "doc") return t("library.type_doc", "文档");
  switch (e.artifactKind) {
    case "image": return t("library.kind_image", "图片");
    case "video": return t("library.kind_video", "视频");
    case "audio": return t("library.kind_audio", "音频");
    case "pdf": return t("library.kind_pdf", "PDF");
    case "doc": return t("library.kind_doc", "Word");
    case "sheet": return t("library.kind_sheet", "表格");
    case "slides": return t("library.kind_slides", "演示");
    case "archive": return t("library.kind_archive", "压缩包");
    case "text": return t("library.kind_text", "文本");
    default: return t("library.type_artifact", "产出");
  }
}

function groupLabel(t: TFunction, g: LibraryTypeGroup): string {
  switch (g) {
    case "doc": return t("library.group_doc", "文档");
    case "artifact": return t("library.group_artifact", "产出");
    case "text": return t("library.group_text", "文稿");
    case "image": return t("library.group_image", "图片");
    case "video": return t("library.group_video", "视频");
    case "sheet": return t("library.group_sheet", "表格");
    case "slides": return t("library.group_slides", "演示");
    case "audio": return t("library.group_audio", "音频");
    default: return t("library.group_other", "其他");
  }
}

/** What the content column needs from the page. */
export interface ContentCtx {
  selectedKey: string | null;
  draggingKey: string | null;
  onOpen: (e: LibraryEntry) => void;
  entryDrag: (e: LibraryEntry) => { onPointerDown: (ev: React.PointerEvent) => void };
}

/** One export to a zip, as the page runs it (`exportLibraryZip`). */
export interface ExportRequest {
  /** What is exporting, for its button's spinner: a folder path, "" for the
   *  unfiled bucket, "selection" or "project". */
  key: string;
  scope: "folder" | "items" | "project";
  folder?: string;
  items?: string[];
  /** The archive's name, without `.zip`. */
  name: string;
}

/** The upload / re-file / export verbs of the open project. */
export interface ContentVerbs {
  onUpload: (kind: PickKind) => void;
  uploading: { sent: number; total: number } | null;
  onRefile: () => void;
  refiling: boolean;
  onExport: (req: ExportRequest) => void;
  /** The key of the export running, if any. */
  exporting: string | null;
}

export type ContentMode = "search" | "batches" | "overview" | "folder";

/**
 * The 资料 page's middle column: the open project's overview (folder cards),
 * one folder's items as cards with summary, tags and a thumbnail, the search
 * results in the same cards, or the project's upload / re-file history.
 */
export function LibraryContent(props: {
  mode: ContentMode;
  project: LibraryProjectTree | null;
  view: LibraryTreeView | null;
  folder: string | null;
  /** The open folder's items with summaries (`library_folder_items`); null
   *  while they load. The tree itself is an outline. */
  folderItems: LibraryEntry[] | null;
  /** A one-character query filters the listing on titles. */
  query: string;
  typeGroup: LibraryTypeGroup | null;
  sort: LibrarySort;
  onTypeGroup: (g: LibraryTypeGroup | null) => void;
  onSort: (s: LibrarySort) => void;
  onPickFolder: (path: string | null) => void;
  onAcceptFolder: (project: string, path: string) => void;
  hits: LibraryHit[] | null;
  searchAll: boolean;
  onSearchAll: (all: boolean) => void;
  /** Every project's entries by `libraryEntryKey`, for a hit's thumbnail. */
  entryByKey: Map<string, LibraryEntry>;
  projectName: (workspacePath: string) => string;
  batches: IngestBatchSummary[];
  onOpenBatch: (id: string) => void;
  ctx: ContentCtx;
  verbs: ContentVerbs;
  /** The preview column is open: the cards stack in one column. */
  narrow: boolean;
}) {
  const { mode } = props;
  if (mode === "search") return <SearchResults {...props} />;
  if (mode === "batches") return <BatchList {...props} />;
  if (!props.project || !props.view) return <SkeletonList rows={6} />;
  if (mode === "overview") return <Overview {...props} project={props.project} view={props.view} />;
  // Keyed by folder: a selection belongs to the listing it was made in.
  return <FolderContent key={props.folder ?? ""} {...props} project={props.project} view={props.view} />;
}

type Props = Parameters<typeof LibraryContent>[0];

function UploadVerbs({ verbs, showRefile }: { verbs: ContentVerbs; showRefile: boolean }) {
  const { t } = useTranslation();
  return (
    <span className={styles.content_verbs}>
      {showRefile && (
        <button
          type="button"
          className={styles.upload_btn}
          title={t("library.refile_hint", "让 AI 把未归档的资料放进合适的目录（先确认数量和费用，可撤销）")}
          onClick={verbs.onRefile}
          disabled={verbs.refiling}
          data-refile
        >
          {verbs.refiling ? <Spinner size={11} /> : <FolderSync size={13} strokeWidth={1.6} />}
          {t("library.refile", "整理存量")}
        </button>
      )}
      {verbs.uploading && (
        <span className={styles.backfill}>
          <Spinner size={11} />
          {t("library.browser_uploading", "正在上传 {{sent}}/{{total}}", verbs.uploading)}
        </span>
      )}
      <button
        type="button"
        className={styles.upload_btn}
        title={t("library.upload_files_hint", "上传文件或压缩包，AI 自动归档")}
        onClick={() => verbs.onUpload("files")}
        disabled={verbs.uploading !== null}
        data-library-upload-btn
      >
        <FileUp size={13} strokeWidth={1.6} />
        {t("library.upload_files", "上传")}
      </button>
      <button
        type="button"
        className={styles.upload_btn}
        title={t("library.upload_folder", "上传文件夹")}
        aria-label={t("library.upload_folder", "上传文件夹")}
        onClick={() => verbs.onUpload("folder")}
        disabled={verbs.uploading !== null}
      >
        <FolderUp size={13} strokeWidth={1.6} />
      </button>
    </span>
  );
}

function ExportButton({
  label,
  hint,
  running,
  disabled,
  onClick,
  testId,
}: {
  label: string;
  hint: string;
  running: boolean;
  disabled: boolean;
  onClick: () => void;
  testId: string;
}) {
  return (
    <button
      type="button"
      className={styles.upload_btn}
      title={hint}
      onClick={onClick}
      disabled={disabled || running}
      data-library-export={testId}
    >
      {running ? <Spinner size={11} /> : <Download size={13} strokeWidth={1.6} />}
      {label}
    </button>
  );
}

// ── Overview ────────────────────────────────────────────────────────────────

function Overview(props: Props & { project: LibraryProjectTree; view: LibraryTreeView }) {
  const { t } = useTranslation();
  const { project, view, verbs, onPickFolder } = props;
  const cards = useMemo(() => folderCards(view, 3), [view]);
  const review = useMemo(() => foldersAwaitingReview(view.folders), [view]);
  const folderTotal = useMemo(() => {
    let n = 0;
    const walk = (ns: LibraryFolderNode[]) => {
      for (const f of ns) {
        n += 1;
        walk(f.children);
      }
    };
    walk(view.folders);
    return n;
  }, [view]);
  const unfiledRecent = view.unfiled.slice(0, 3);
  const unfiled = view.unfiledTotal;
  return (
    <div className={styles.content_scroll} data-library-content="overview">
      <div className={styles.content_head}>
        <div className={styles.content_title_row}>
          <h2 className={styles.content_title}>{project.workspaceName}</h2>
          <span className={styles.content_verbs}>
            <ExportButton
              label={t("library.export_project", "导出整个项目")}
              hint={t("library.export_project_hint", "把这个项目的全部文档和产出（含未归档）按目录结构打成一个 zip")}
              running={verbs.exporting === "project"}
              disabled={verbs.exporting !== null || projectTotals(project).total === 0}
              onClick={() =>
                verbs.onExport({ key: "project", scope: "project", name: project.workspaceName })
              }
              testId="project"
            />
            <UploadVerbs verbs={verbs} showRefile={unfiled > 0} />
          </span>
        </div>
        <p className={styles.content_desc}>
          {t("library.overview_stats", "{{items}} 条资料 · {{folders}} 个目录 · 未归档 {{unfiled}} 条", {
            items: projectTotals(project).total,
            folders: folderTotal,
            unfiled,
          })}
        </p>
      </div>

      {review.length > 0 && (
        <section className={styles.review} data-library-review>
          <h3 className={styles.section_title}>
            <Sparkles size={13} strokeWidth={1.8} />
            {t("library.review_head", "待你过目的 AI 新建目录")}
            <span className={styles.count}>{review.length}</span>
          </h3>
          <p className={styles.section_hint}>
            {t("library.review_hint", "AI 归档时觉得没有合适的目录，就新建了这些。合适就保留，不合适可以在左栏重命名或删除。")}
          </p>
          {review.map((f) => (
            <div key={f.path} className={styles.review_row}>
              <button type="button" className={styles.review_main} onClick={() => onPickFolder(f.path)}>
                <Folder size={14} strokeWidth={1.5} />
                <span className={styles.review_name}>{f.path}</span>
                <span className={styles.count}>{f.total}</span>
                {f.origin?.reason && (
                  <span className={styles.review_reason} title={originTitle(t, f.origin)}>
                    {f.origin.reason}
                  </span>
                )}
              </button>
              <button
                type="button"
                className={styles.ai_accept}
                title={t("library.ai_folder_accept_hint", "保留这个文件夹（去掉标记）；不要就重命名或删除")}
                onClick={() => props.onAcceptFolder(project.workspacePath, f.path)}
              >
                <Check size={11} strokeWidth={2} />
                {t("library.ai_folder_accept", "保留")}
              </button>
            </div>
          ))}
        </section>
      )}

      {cards.length === 0 && unfiled === 0 ? (
        <div className={styles.none}>
          {t("library.overview_empty", "这个项目还没有资料。点「上传」，或把文件拖到这里——AI 会读内容、写摘要、放进合适的目录。")}
        </div>
      ) : (
        <div className={`${styles.folder_grid}${props.narrow ? ` ${styles.folder_grid_narrow}` : ""}`}>
          {cards.map(({ node, recent }) => (
            <FolderCardView key={node.path} node={node} recent={recent} onOpen={() => onPickFolder(node.path)} />
          ))}
          {unfiled > 0 && (
            <button
              type="button"
              className={styles.folder_card}
              onClick={() => onPickFolder("")}
              data-library-card-folder=""
            >
              <span className={styles.folder_card_head}>
                <Inbox size={16} strokeWidth={1.5} />
                <span className={styles.folder_card_name}>{t("library.unfiled", "未归档")}</span>
                <span className={styles.count}>{unfiled}</span>
              </span>
              <span className={styles.folder_card_desc}>
                {t("library.unfiled_desc", "不在任何目录里的资料，拖进文件夹即可归档")}
              </span>
              <RecentList entries={unfiledRecent} />
            </button>
          )}
        </div>
      )}
    </div>
  );
}

function FolderCardView({
  node,
  recent,
  onOpen,
}: {
  node: LibraryFolderNode;
  recent: LibraryEntry[];
  onOpen: () => void;
}) {
  const { t } = useTranslation();
  const awaiting = !!node.origin && !node.origin.acceptedMs;
  return (
    <button type="button" className={styles.folder_card} onClick={onOpen} data-library-card-folder={node.path}>
      <span className={styles.folder_card_head}>
        <Folder size={16} strokeWidth={1.5} />
        <span className={styles.folder_card_name}>{node.label}</span>
        {awaiting && node.origin && (
          <span className={styles.ai_chip} title={originTitle(t, node.origin)}>
            <Sparkles size={10} strokeWidth={1.8} />
            {t("library.ai_folder_agent", "AI 新建")}
          </span>
        )}
        <span className={styles.count}>{node.total}</span>
      </span>
      <span className={node.description ? styles.folder_card_desc : `${styles.folder_card_desc} ${styles.dim}`}>
        {node.description || t("library.no_description", "还没有说明")}
      </span>
      {node.children.length > 0 && (
        <span className={styles.folder_card_subs}>
          {node.children.slice(0, 4).map((c) => (
            <span key={c.path} className={styles.sub_chip}>
              {c.label} <span className={styles.count}>{c.total}</span>
            </span>
          ))}
          {node.children.length > 4 && (
            <span className={styles.sub_chip}>+{node.children.length - 4}</span>
          )}
        </span>
      )}
      <RecentList entries={recent} />
    </button>
  );
}

function RecentList({ entries }: { entries: LibraryEntry[] }) {
  const { t } = useTranslation();
  if (entries.length === 0) {
    return <span className={`${styles.folder_card_recent} ${styles.dim}`}>{t("library.folder_empty", "还没有资料")}</span>;
  }
  return (
    <span className={styles.folder_card_recent}>
      {entries.map((e) => {
        const Icon = iconOf(e);
        return (
          <span key={libraryEntryKey(e)} className={styles.recent_row}>
            <Icon size={12} strokeWidth={1.6} />
            <span className={styles.recent_title}>{e.title}</span>
            {e.updatedMs > 0 && <span className={styles.time}>{timeAgo(e.updatedMs, t)}</span>}
          </span>
        );
      })}
    </span>
  );
}

// ── One folder ──────────────────────────────────────────────────────────────

function FolderContent(props: Props & { project: LibraryProjectTree; view: LibraryTreeView }) {
  const { t } = useTranslation();
  const { project, view, folder, folderItems, typeGroup, sort, verbs, onPickFolder } = props;
  const node = folder ? findLibraryFolder(view.folders, folder) : null;
  const rows = useMemo<FolderListing[]>(() => {
    const all = listingOf(folderItems ?? [], folder ?? "");
    const q = props.query.trim();
    return q ? all.filter((r) => libraryEntryMatches(r.entry, q)) : all;
  }, [folderItems, folder, props.query]);
  const loading = folderItems === null;
  // Until the listing arrives the outline's count stands in.
  const total = loading ? (node ? node.total : view.unfiledTotal) : rows.length;
  const counts = useMemo(() => typeCounts(rows), [rows]);
  const chips = useMemo(() => visibleTypeGroups(counts), [counts]);
  // A filter with no chip here (its bucket emptied, or it would select
  // everything) falls back to all.
  const group = typeGroup && chips.includes(typeGroup) ? typeGroup : null;
  const shown = useMemo(() => filterListing(rows, group, sort), [rows, group, sort]);

  // Multi-select for "导出所选": while on, a card click toggles it instead
  // of opening it, and cards are not draggable.
  const [selecting, setSelecting] = useState(false);
  const [picked, setPicked] = useState<Set<string>>(() => new Set());
  const toggle = useCallback((key: string) => {
    setPicked((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  }, []);
  const stopSelecting = () => {
    setSelecting(false);
    setPicked(new Set());
  };
  const shownKeys = shown.map((r) => libraryEntryKey(r.entry));
  const allShownPicked = shownKeys.length > 0 && shownKeys.every((k) => picked.has(k));

  const crumbs = folder ? folder.split("/") : [];
  const label = node ? node.label : t("library.unfiled", "未归档");
  const description = node ? node.description : t("library.unfiled_desc", "不在任何目录里的资料，拖进文件夹即可归档");

  return (
    <div className={styles.content_scroll} data-library-content="folder">
      <div className={styles.content_head}>
        <div className={styles.crumbs}>
          <button type="button" className={styles.crumb} onClick={() => onPickFolder(null)}>
            {project.workspaceName}
          </button>
          {crumbs.slice(0, -1).map((seg, i) => {
            const path = crumbs.slice(0, i + 1).join("/");
            return (
              <span key={path} className={styles.crumb_seg}>
                <ChevronRight size={12} />
                <button type="button" className={styles.crumb} onClick={() => onPickFolder(path)}>
                  {seg}
                </button>
              </span>
            );
          })}
        </div>
        <div className={styles.content_title_row}>
          <h2 className={styles.content_title}>
            {label}
            <span className={styles.content_count}>{total}</span>
          </h2>
          <span className={styles.content_verbs}>
            <ExportButton
              label={t("library.export_folder", "导出")}
              hint={t("library.export_folder_hint", "把这个目录（含子目录）的文档和产出按目录结构打成一个 zip")}
              running={verbs.exporting === (folder ?? "")}
              // The unfiled bucket is no folder: it goes out as the listed items.
              disabled={verbs.exporting !== null || total === 0 || (!node && loading)}
              onClick={() =>
                verbs.onExport(
                  node
                    ? { key: node.path, scope: "folder", folder: node.path, name: label }
                    : { key: "", scope: "items", folder: "", items: rows.map((r) => libraryEntryKey(r.entry)), name: label },
                )
              }
              testId="folder"
            />
            <UploadVerbs verbs={verbs} showRefile={!node && view.unfiledTotal > 0} />
          </span>
        </div>
        {description && <p className={styles.content_desc}>{description}</p>}
        {node && node.children.length > 0 && (
          <div className={styles.sub_folders}>
            {node.children.map((c) => (
              <button key={c.path} type="button" className={styles.sub_chip_btn} onClick={() => onPickFolder(c.path)}>
                <Folder size={12} strokeWidth={1.5} />
                {c.label}
                <span className={styles.count}>{c.total}</span>
              </button>
            ))}
          </div>
        )}
        {rows.length > 0 && (
          <div className={styles.content_tools}>
            <div className={styles.type_chips} role="tablist">
              <button
                type="button"
                role="tab"
                aria-selected={group === null}
                className={`${styles.type_chip}${group === null ? ` ${styles.type_chip_on}` : ""}`}
                onClick={() => props.onTypeGroup(null)}
              >
                {t("library.group_all", "全部")} <span className={styles.count}>{rows.length}</span>
              </button>
              {chips.map((g, i) => (
                <span key={g} className={styles.type_chip_slot}>
                  {/* A hairline between the source chips and the kind chips. */}
                  {i > 0 && LIBRARY_SOURCE_GROUPS.includes(chips[i - 1]) && !LIBRARY_SOURCE_GROUPS.includes(g) && (
                    <span className={styles.type_chip_sep} aria-hidden />
                  )}
                  <button
                    type="button"
                    role="tab"
                    aria-selected={group === g}
                    className={`${styles.type_chip}${group === g ? ` ${styles.type_chip_on}` : ""}`}
                    onClick={() => props.onTypeGroup(g)}
                    data-library-type={g}
                  >
                    {groupLabel(t, g)} <span className={styles.count}>{counts[g]}</span>
                  </button>
                </span>
              ))}
            </div>
            <button
              type="button"
              className={`${styles.type_chip}${selecting ? ` ${styles.type_chip_on}` : ""}`}
              onClick={() => (selecting ? stopSelecting() : setSelecting(true))}
              title={t("library.select_hint", "勾选几条资料，一起导出")}
              aria-pressed={selecting}
              data-library-select
            >
              <ListChecks size={12} strokeWidth={1.7} />
              {t("library.select", "多选")}
            </button>
            <select
              className={styles.select}
              value={sort}
              onChange={(e) => props.onSort(e.target.value as LibrarySort)}
              aria-label={t("library.sort", "排序")}
            >
              <option value="newest">{t("library.sort_newest", "最新在前")}</option>
              <option value="oldest">{t("library.sort_oldest", "最早在前")}</option>
            </select>
          </div>
        )}
      </div>
      {selecting && (
        <div className={styles.select_bar} data-library-select-bar>
          <span className={styles.select_count}>
            {t("library.selected_n", "已选 {{n}} 项", { n: picked.size })}
          </span>
          <button
            type="button"
            className={styles.select_link}
            onClick={() => setPicked(allShownPicked ? new Set() : new Set(shownKeys))}
          >
            {allShownPicked ? t("library.select_none", "全不选") : t("library.select_all", "全选")}
          </button>
          <span className={styles.select_spacer} />
          <ExportButton
            label={t("library.export_selected", "导出所选")}
            hint={t("library.export_selected_hint", "把勾选的资料打成一个 zip，保留它们的目录结构")}
            running={verbs.exporting === "selection"}
            disabled={verbs.exporting !== null || picked.size === 0}
            onClick={() =>
              verbs.onExport({
                key: "selection",
                scope: "items",
                folder: folder ?? "",
                items: [...picked],
                name: t("library.export_selection_name", "{{name}}（{{n}} 项）", { name: label, n: picked.size }),
              })
            }
            testId="selection"
          />
          <button
            type="button"
            className={styles.select_close}
            onClick={stopSelecting}
            title={t("library.select_done", "退出多选")}
            aria-label={t("library.select_done", "退出多选")}
          >
            <X size={13} strokeWidth={1.8} />
          </button>
        </div>
      )}
      {loading ? (
        <SkeletonList rows={Math.min(Math.max(total, 2), 6)} />
      ) : rows.length === 0 ? (
        <div className={styles.none}>
          {node && node.children.length === 0
            ? t("library.folder_empty_hint", "这个目录还没有资料。把条目从别处拖到左栏的这个目录上，或上传文件。")
            : t("library.no_match", "没有符合筛选条件的资料")}
        </div>
      ) : (
        <div className={`${styles.cards}${props.narrow ? ` ${styles.cards_narrow}` : ""}`}>
          {shown.map(({ entry, sub }) => {
            const key = libraryEntryKey(entry);
            return (
              <ItemCard
                key={key}
                entry={entry}
                where={sub}
                ctx={props.ctx}
                select={selecting ? { checked: picked.has(key), onToggle: () => toggle(key) } : undefined}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

// ── Search ──────────────────────────────────────────────────────────────────

function SearchResults(props: Props) {
  const { t } = useTranslation();
  const { hits, searchAll, entryByKey, ctx } = props;
  return (
    <div className={styles.content_scroll} data-library-content="search">
      <div className={styles.content_head}>
        <div className={styles.content_title_row}>
          <h2 className={styles.content_title}>
            {t("library.search_head", "搜索「{{q}}」", { q: props.query.trim() })}
            {hits && <span className={styles.content_count}>{hits.length}</span>}
          </h2>
          <div className={styles.segmented} role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={!searchAll}
              className={`${styles.type_chip}${!searchAll ? ` ${styles.type_chip_on}` : ""}`}
              onClick={() => props.onSearchAll(false)}
              disabled={!props.project}
            >
              {props.project?.workspaceName ?? t("library.scope_project", "本项目")}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={searchAll}
              className={`${styles.type_chip}${searchAll ? ` ${styles.type_chip_on}` : ""}`}
              onClick={() => props.onSearchAll(true)}
            >
              {t("library.project_all", "全部项目")}
            </button>
          </div>
        </div>
      </div>
      {hits === null ? (
        <SkeletonList rows={6} />
      ) : hits.length === 0 ? (
        <div className={styles.none}>{t("library.no_hits", "没有找到相关资料")}</div>
      ) : (
        <div className={`${styles.cards}${props.narrow ? ` ${styles.cards_narrow}` : ""}`}>
          {hits.map((h) => {
            const key = libraryEntryKey(h);
            // The hit carries the file facts a thumbnail needs; a loaded
            // entry is used when there is one.
            const known = entryByKey.get(key);
            const entry: LibraryEntry = known ?? {
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
            const folderText = h.folder || t("library.unfiled", "未归档");
            const where = searchAll ? `${props.projectName(h.workspacePath)} / ${folderText}` : folderText;
            return (
              <ItemCard
                key={key}
                entry={{ ...entry, title: h.title, summary: h.summary, tags: h.tags }}
                where={where}
                snippet={h.snippet || h.summary}
                hitInSummary={h.field === "summary"}
                ctx={ctx}
              />
            );
          })}
        </div>
      )}
    </div>
  );
}

// ── Batches ─────────────────────────────────────────────────────────────────

function BatchList(props: Props) {
  const { t } = useTranslation();
  const running = props.batches.filter((b) => isIngestRunning(b.status)).length;
  return (
    <div className={styles.content_scroll} data-library-content="batches">
      <div className={styles.content_head}>
        <div className={styles.content_title_row}>
          <h2 className={styles.content_title}>
            {t("library.batches", "整理记录")}
            <span className={styles.content_count}>{props.batches.length}</span>
          </h2>
        </div>
        <p className={styles.content_desc}>
          {running > 0
            ? t("library.batches_desc_running", "{{n}} 批正在整理。每一批上传或整理存量都记在这里，点开可以逐条调整或整批撤销。", { n: running })
            : t("library.batches_desc", "每一批上传或整理存量都记在这里，点开可以逐条调整或整批撤销。")}
        </p>
      </div>
      <div className={styles.batch_list}>
        {props.batches.map((b) => (
          <BatchRow
            key={b.id}
            batch={b}
            depth={0}
            active={props.ctx.selectedKey === ingestBatchKey(b.id)}
            onOpen={() => props.onOpenBatch(b.id)}
          />
        ))}
      </div>
    </div>
  );
}

// ── Cards ───────────────────────────────────────────────────────────────────

function ItemCard({
  entry,
  where,
  snippet,
  hitInSummary = false,
  ctx,
  select,
}: {
  entry: LibraryEntry;
  /** The subfolder (in a folder listing) or the folder (in search results). */
  where: string;
  /** A search snippet with `<mark>` runs; the summary otherwise. */
  snippet?: string;
  hitInSummary?: boolean;
  ctx: ContentCtx;
  /** Multi-select is on: a click toggles the card instead of opening it. */
  select?: { checked: boolean; onToggle: () => void };
}) {
  const { t } = useTranslation();
  const key = libraryEntryKey(entry);
  const active = !select && ctx.selectedKey === key;
  const text = snippet ?? entry.summary ?? "";
  const tags = entry.tags ?? [];
  // Only a picture earns the thumbnail well; everything else gets a small
  // kind icon by its title, so a folder of documents is not a column of
  // empty squares.
  const media = thumbMediaOf(entry);
  const Icon = iconOf(entry);
  return (
    <button
      type="button"
      className={`${styles.card}${active ? ` ${styles.card_active}` : ""}${
        ctx.draggingKey === key ? ` ${styles.row_dragging}` : ""
      }${select?.checked ? ` ${styles.card_checked}` : ""}`}
      onClick={() => (select ? select.onToggle() : ctx.onOpen(entry))}
      aria-current={active ? "true" : undefined}
      aria-pressed={select ? select.checked : undefined}
      data-library-item={key}
      {...(select ? {} : ctx.entryDrag(entry))}
    >
      {select && (
        <span className={styles.card_check} data-checked={select.checked || undefined} aria-hidden>
          {select.checked && <Check size={11} strokeWidth={2.4} />}
        </span>
      )}
      {media && <Thumb entry={entry} media={media} />}
      <span className={styles.card_body}>
        <span className={styles.card_title_row}>
          {!media && (
            <span className={styles.card_icon} data-type={entry.kind}>
              <Icon size={14} strokeWidth={1.6} />
            </span>
          )}
          <span className={styles.card_title}>{entry.title}</span>
        </span>
        {text ? (
          <span className={styles.card_summary}>
            {snippetRuns(text).map((r, i) => (r.mark ? <mark key={i}>{r.text}</mark> : <span key={i}>{r.text}</span>))}
          </span>
        ) : (
          <span className={`${styles.card_summary} ${styles.dim}`}>{t("library.no_summary", "还没有摘要")}</span>
        )}
        {tags.length > 0 && (
          <span className={styles.card_tags}>
            {tags.slice(0, 6).map((tag) => (
              <span key={tag} className={styles.tag}>{tag}</span>
            ))}
          </span>
        )}
        <span className={styles.card_meta}>
          <span className={styles.card_kind}>{kindLabel(t, entry)}</span>
          {where && (
            <span className={styles.location} title={where}>
              <Folder size={11} aria-hidden style={{ flexShrink: 0 }} />
              <span className={styles.location_text}>{where}</span>
            </span>
          )}
          {hitInSummary && <span>{t("library.hit_in_summary", "摘要命中")}</span>}
          {entry.updatedMs > 0 && <span className={styles.time}>{timeAgo(entry.updatedMs, t)}</span>}
        </span>
      </span>
    </button>
  );
}

/** Which picture a card can show: an image's own bytes or a video's poster
 *  frame; null for every other kind. */
function thumbMediaOf(entry: LibraryEntry): "image" | "video" | null {
  if (entry.kind !== "artifact" || !entry.name) return null;
  if (entry.artifactKind === "image") return "image";
  if (entry.artifactKind === "video" && thumbMode(entry.mime ?? "", entry.sizeBytes ?? 0) === "video") return "video";
  return null;
}

/** The thumbnail well of a picture card; the kind icon shows through when
 *  loading fails — a thumbnail is decoration. */
function Thumb({ entry, media }: { entry: LibraryEntry; media: "image" | "video" }) {
  const [failed, setFailed] = useState(false);
  // A video first tries the host's poster frame — the same one the phone
  // shows — and only without one grabs a frame in the webview.
  const poster = media === "video" ? artifactPosterUrl(entry.id, entry.name ?? "") : null;
  const [posterFailed, setPosterFailed] = useState(poster === null);
  // Stable: ArtifactThumb re-runs its render effect when this changes.
  const onFail = useCallback(() => setFailed(true), []);
  const onPosterFail = useCallback(() => setPosterFailed(true), []);
  const Icon = iconOf(entry);
  const url = failed ? "" : artifactBlobUrl(entry.id, entry.name ?? "");
  return (
    <span className={styles.thumb} data-type={entry.kind} data-media={failed ? undefined : media}>
      <Icon size={18} strokeWidth={1.5} />
      {!failed && media === "image" && (
        <img src={url} alt="" loading="lazy" decoding="async" draggable={false} onError={onFail} />
      )}
      {!failed && media === "video" && !posterFailed && poster && (
        <img src={poster} alt="" loading="lazy" decoding="async" draggable={false} onError={onPosterFail} />
      )}
      {!failed && media === "video" && posterFailed && (
        <ArtifactThumb
          id={entry.id}
          url={url}
          mode="video"
          title={entry.title}
          sizeBytes={entry.sizeBytes ?? 0}
          onFail={onFail}
        />
      )}
    </span>
  );
}
