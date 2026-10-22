import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  Check,
  ChevronDown,
  ChevronRight,
  Folder,
  FolderGit2,
  FolderOpen,
  FolderPlus,
  History,
  Inbox,
  LayoutGrid,
  NotebookPen,
  Pencil,
  Sparkles,
  Trash2,
} from "lucide-react";

import {
  folderAwaitingReview,
  libraryDropKey,
  type LibraryFolderNode,
  type LibraryProjectTree,
  type LibraryTreeView,
} from "../../../shared-ts/libraryTree";
import { projectTotals } from "../../../shared-ts/libraryBrowse";
import { Spinner } from "./loading";
import styles from "./LibraryView.module.css";

/** Rows carrying this attribute are drop zones; the value is a
 *  `libraryDropKey(project, folder)` (`folder: ""` = the unfiled bucket). */
export const DROP_ATTR = "data-library-drop";

/** A project row carries its key here: files dropped from the OS onto it are
 *  uploaded into that project. */
export const UPLOAD_ATTR = "data-library-upload";

/** What the rail's folder rows need from the page, passed down once. */
export interface FolderRailCtx {
  project: string;
  /** The folder the content column lists; null = overview, "" = unfiled. */
  folder: string | null;
  /** Overview / folder rows are not "current" while a search or the batch
   *  list owns the content column. */
  contentElsewhere: boolean;
  dropActive: string | null;
  busy: string | null;
  onPickFolder: (path: string | null) => void;
  onCreateFolder: (project: string, path: string) => void;
  onRenameFolder: (project: string, from: string, to: string) => void;
  onDeleteFolder: (project: string, path: string, label: string) => void;
  onDescribeFolder: (project: string, path: string, description: string) => void;
  onAcceptFolder: (project: string, path: string) => void;
}

/** Hover text of an AI-created folder's marker: who made it, when, and why. */
export function originTitle(
  t: (k: string, d: string, o?: Record<string, unknown>) => string,
  o: NonNullable<LibraryFolderNode["origin"]>,
): string {
  const who =
    o.source === "upload"
      ? t("library.ai_folder_by_upload", "上传整理时由 AI 新建（批次 {{id}}）", { id: o.batchId ?? "" })
      : o.source === "refile"
        ? t("library.ai_folder_by_refile", "整理存量时由 AI 新建（批次 {{id}}）", { id: o.batchId ?? "" })
        : t("library.ai_folder_by_agent", "由 agent 归档时新建（会话 {{id}}）", { id: (o.sessionId ?? "").slice(0, 8) });
  const when = o.createdMs ? new Date(o.createdMs).toLocaleString() : "";
  return [who, when, o.reason ? t("library.ai_folder_reason", "原因：{{reason}}", { reason: o.reason }) : ""]
    .filter(Boolean)
    .join("\n");
}

/**
 * The 资料 page's left column: every project as one row, and under the open
 * one its overview entry, its folder tree (with counts, never the items
 * themselves — those are the content column's), the unfiled bucket and the
 * batch history. A project's whole schema fits on one screen this way, which
 * a tree that also listed every item never could.
 */
export function LibraryFolderRail({
  projects,
  active,
  view,
  ctx,
  batchCount,
  batchesRunning,
  showBatches,
  dropProject,
  onPickProject,
  onShowBatches,
}: {
  projects: LibraryProjectTree[];
  active: string;
  /** The open project's nested tree. */
  view: LibraryTreeView | null;
  ctx: FolderRailCtx;
  batchCount: number;
  batchesRunning: number;
  showBatches: boolean;
  dropProject: string | null;
  onPickProject: (path: string) => void;
  onShowBatches: () => void;
}) {
  const { t } = useTranslation();
  const [creating, setCreating] = useState(false);
  const rootBusy = ctx.busy === libraryDropKey(ctx.project, "") && !creating;

  // Keep the open project on screen. With thirty-odd projects the one the page
  // reopens on (or the one picked low in the list, whose predecessor folds
  // above it) can sit out of view; when its row and folder tree are not wholly
  // visible, scroll its row to the top of the rail. Only when the open project
  // changes — a reload under a folder edit must not move the rail.
  const openRef = useRef<HTMLDivElement>(null);
  const scrolledFor = useRef<string | null>(null);
  const hasView = view !== null;
  useLayoutEffect(() => {
    const el = openRef.current;
    if (!el || !hasView || scrolledFor.current === active) return;
    const first = scrolledFor.current === null;
    scrolledFor.current = active;
    const scroller = scrollParent(el);
    if (!scroller) return;
    const box = scroller.getBoundingClientRect();
    const r = el.getBoundingClientRect();
    if (r.top >= box.top && r.bottom <= box.bottom) return;
    scroller.scrollTo({
      top: scroller.scrollTop + r.top - box.top - 4,
      behavior: first ? "auto" : "smooth",
    });
  }, [active, hasView]);

  return (
    <>
      {projects.map((p) => {
        const open = p.workspacePath === active;
        const drop = dropProject === p.workspacePath;
        return (
          <div
            key={p.workspacePath}
            ref={open ? openRef : undefined}
            className={`${styles.rail_project}${open ? ` ${styles.rail_project_open}` : ""}${drop ? ` ${styles.project_drop}` : ""}`}
            {...{ [UPLOAD_ATTR]: p.workspacePath }}
          >
            <div className={styles.tree_row}>
              <button
                type="button"
                className={styles.tree_target}
                title={p.workspacePath}
                onClick={() => onPickProject(p.workspacePath)}
                data-library-project={p.workspaceName}
              >
                {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
                <FolderGit2 size={15} strokeWidth={1.5} />
                <span className={styles.project_name}>{p.workspaceName}</span>
                <span className={styles.count}>{projectTotals(p).total}</span>
              </button>
              {open && (
                <span className={styles.actions}>
                  <button
                    type="button"
                    className={styles.action}
                    title={t("library.folder_new", "新建文件夹")}
                    aria-label={t("library.folder_new", "新建文件夹")}
                    onClick={() => setCreating(true)}
                  >
                    <FolderPlus size={13} strokeWidth={1.5} />
                  </button>
                </span>
              )}
            </div>
            {open && view && (
              <div className={styles.rail_tree}>
                <button
                  type="button"
                  className={`${styles.tree_row} ${styles.rail_row}${
                    ctx.folder === null && !ctx.contentElsewhere ? ` ${styles.rail_row_active}` : ""
                  }`}
                  onClick={() => ctx.onPickFolder(null)}
                  data-library-overview
                >
                  <LayoutGrid size={14} strokeWidth={1.5} />
                  <span className={styles.folder_label}>{t("library.overview", "概览")}</span>
                </button>
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
                {rootBusy && (
                  <div className={styles.row_busy}>
                    <Spinner size={12} />
                  </div>
                )}
                {view.folders.map((f) => (
                  <FolderNode key={f.path} node={f} depth={0} ctx={ctx} />
                ))}
                {view.folders.length === 0 && (
                  <div className={styles.no_folders}>
                    {t("library.no_folders_rail", "还没有目录。点上方 + 新建，或上传文件让 AI 起草一套。")}
                  </div>
                )}
                <UnfiledRow count={view.unfiledTotal} ctx={ctx} />
                {batchCount > 0 && (
                  <button
                    type="button"
                    className={`${styles.tree_row} ${styles.rail_row}${showBatches ? ` ${styles.rail_row_active}` : ""}`}
                    onClick={onShowBatches}
                    data-library-batches
                  >
                    {batchesRunning > 0 ? <Spinner size={12} /> : <History size={14} strokeWidth={1.5} />}
                    <span className={styles.folder_label}>{t("library.batches", "整理记录")}</span>
                    <span className={styles.count}>
                      {batchesRunning > 0
                        ? t("library.batches_running", "{{n}} 进行中", { n: batchesRunning })
                        : batchCount}
                    </span>
                  </button>
                )}
              </div>
            )}
          </div>
        );
      })}
    </>
  );
}

/** The nearest ancestor that scrolls vertically. */
function scrollParent(el: HTMLElement): HTMLElement | null {
  for (let p = el.parentElement; p; p = p.parentElement) {
    const oy = getComputedStyle(p).overflowY;
    if ((oy === "auto" || oy === "scroll") && p.scrollHeight > p.clientHeight) return p;
  }
  return null;
}

/** Whether `path` is `folder` or one of its ancestors. */
function holds(path: string, folder: string | null): boolean {
  return !!folder && (folder === path || folder.startsWith(`${path}/`));
}

function FolderNode({ node, depth, ctx }: { node: LibraryFolderNode; depth: number; ctx: FolderRailCtx }) {
  const { t } = useTranslation();
  // Subfolders start folded so the top level reads at a glance; the branch
  // holding the folder on screen opens itself.
  const [expanded, setExpanded] = useState(() => holds(node.path, ctx.folder) && ctx.folder !== node.path);
  const [editing, setEditing] = useState<null | "create" | "rename" | "describe">(null);
  const key = libraryDropKey(ctx.project, node.path);
  const rowBusy = ctx.busy === key;
  const review = folderAwaitingReview(node.origin) ? node.origin : null;
  const current = ctx.folder === node.path && !ctx.contentElsewhere;
  const hasChildren = node.children.length > 0;

  const holding = holds(node.path, ctx.folder) && ctx.folder !== node.path;
  const wasHolding = useRef(holding);
  useEffect(() => {
    if (holding && !wasHolding.current) setExpanded(true);
    wasHolding.current = holding;
  }, [holding]);

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
        className={`${styles.tree_row}${current ? ` ${styles.rail_row_active}` : ""}${
          ctx.dropActive === key ? ` ${styles.tree_row_drop}` : ""
        }`}
        style={{ paddingLeft: depth * 14 }}
        {...{ [DROP_ATTR]: key }}
      >
        <button
          type="button"
          className={styles.twisty}
          aria-label={expanded ? "Collapse" : "Expand"}
          onClick={() => setExpanded((v) => !v)}
          disabled={!hasChildren}
        >
          {hasChildren ? expanded ? <ChevronDown size={13} /> : <ChevronRight size={13} /> : null}
        </button>
        <button
          type="button"
          className={styles.tree_target}
          onClick={() => ctx.onPickFolder(node.path)}
          title={node.description ? `${node.path}\n${node.description}` : node.path}
          data-library-folder={node.path}
        >
          {current ? <FolderOpen size={15} strokeWidth={1.4} /> : <Folder size={15} strokeWidth={1.4} />}
          <span className={styles.folder_text}>
            <span className={styles.rail_folder_label}>{node.label}</span>
            {review && (
              <span className={styles.ai_dot} title={originTitle(t, review)}>
                <Sparkles size={10} strokeWidth={1.8} />
              </span>
            )}
          </span>
          {rowBusy ? <Spinner size={12} /> : <span className={styles.count}>{node.total}</span>}
        </button>
        <span className={styles.actions}>
          {review && (
            <button
              type="button"
              className={styles.action}
              title={t("library.ai_folder_accept_hint", "保留这个文件夹（去掉标记）；不要就重命名或删除")}
              aria-label={t("library.ai_folder_accept", "保留")}
              onClick={() => ctx.onAcceptFolder(ctx.project, node.path)}
            >
              <Check size={13} strokeWidth={1.8} />
            </button>
          )}
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
          depth={depth + 1}
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
      {expanded &&
        node.children.map((c) => <FolderNode key={c.path} node={c} depth={depth + 1} ctx={ctx} />)}
    </div>
  );
}

function UnfiledRow({ count, ctx }: { count: number; ctx: FolderRailCtx }) {
  const { t } = useTranslation();
  const key = libraryDropKey(ctx.project, "");
  const current = ctx.folder === "" && !ctx.contentElsewhere;
  return (
    <div
      className={`${styles.tree_row} ${styles.unfiled_row}${current ? ` ${styles.rail_row_active}` : ""}${
        ctx.dropActive === key ? ` ${styles.tree_row_drop}` : ""
      }`}
      {...{ [DROP_ATTR]: key }}
    >
      <span className={styles.twisty} />
      <button
        type="button"
        className={styles.tree_target}
        onClick={() => ctx.onPickFolder("")}
        title={t("library.unfiled_desc", "不在任何目录里的资料，拖进文件夹即可归档")}
        data-library-folder=""
      >
        <Inbox size={15} strokeWidth={1.4} />
        <span className={styles.folder_text}>
          <span className={styles.rail_folder_label}>{t("library.unfiled", "未归档")}</span>
        </span>
        <span className={styles.count}>{count}</span>
      </button>
    </div>
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
      style={{ marginLeft: 22 + depth * 14 }}
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
