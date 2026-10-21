// The phone's 资料 as one folder tree per project — the same nesting the
// desktop 资料 page draws (shared-ts/libraryTree.ts). The project list, one
// project's tree with its unfiled bucket, the folder sheet (keep / rename /
// delete an AI-created folder) and the move picker live here; LibraryView
// owns the data and the overlays.

import { useMemo, useState, type ReactNode } from "react";
import {
  ChevronRight,
  FileText,
  Folder,
  FolderInput,
  FolderOpen,
  Inbox,
  MoreHorizontal,
  Sparkles,
  X,
} from "lucide-react";
import { dateLocale, t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { Artifact, WikiDoc } from "../types";
import { formatBytes } from "../artifacts";
import {
  acceptFolder,
  buildLibraryTree,
  deleteFolder,
  folderAwaitingReview,
  folderChoices,
  libraryEntryKey,
  renameFolder,
  type LibraryEntry,
  type LibraryFolderNode,
  type LibraryFolderRec,
  type LibraryProjectTree,
} from "../libraryApi";
import { useConfirm } from "../confirmDialog";
import { KIND_ICON } from "./ArtifactsView";
import { Spinner } from "./loading";
import listStyles from "./LibraryList.module.css";
import styles from "./LibraryTree.module.css";

export type EntryFilter = "all" | "doc" | "artifact";

/** Folders an AI made that nobody has kept, renamed or deleted yet. */
export function awaitingFolders(project: Pick<LibraryProjectTree, "folders">): LibraryFolderRec[] {
  return project.folders.filter((f) => folderAwaitingReview(f.origin));
}

// ── Project list ────────────────────────────────────────────────────────────

export function ProjectList({
  projects,
  onOpen,
}: {
  projects: LibraryProjectTree[];
  onOpen: (workspacePath: string) => void;
}) {
  return (
    <div className={listStyles.list}>
      {projects.map((p) => {
        const review = awaitingFolders(p).length;
        const unfiled = p.items.filter((i) => !i.folder).length;
        return (
          <button
            key={p.workspacePath}
            type="button"
            className={listStyles.row}
            onClick={() => onOpen(p.workspacePath)}
            data-project={p.workspaceName}
          >
            <span className={listStyles.icon}>
              <FolderOpen size={18} strokeWidth={1.5} />
            </span>
            <span className={listStyles.body}>
              <span className={listStyles.line1}>
                <span className={listStyles.title}>{p.workspaceName}</span>
                {review > 0 && (
                  <span className={styles.aiBadge}>
                    <Sparkles size={10} strokeWidth={1.8} />
                    {t("{0} 个新目录待确认", review)}
                  </span>
                )}
              </span>
              <span className={listStyles.line2}>
                {[
                  t("{0} 份资料", p.items.length),
                  t("{0} 个目录", p.folders.length),
                  unfiled > 0 ? t("未归档 {0}", unfiled) : "",
                ]
                  .filter(Boolean)
                  .join(" · ")}
              </span>
            </span>
            <ChevronRight size={16} className={listStyles.chevron} />
          </button>
        );
      })}
    </div>
  );
}

// ── One project's tree ──────────────────────────────────────────────────────

interface TreeProps {
  project: LibraryProjectTree;
  filter: EntryFilter;
  docs: Map<string, WikiDoc>;
  artifacts: Map<string, Artifact>;
  /** Item keys mid-move, drawn with a spinner. */
  busy: ReadonlySet<string>;
  onOpenEntry: (e: LibraryEntry) => void;
  onMoveEntry: (e: LibraryEntry, folder: string) => void;
  onFolderMenu: (folder: LibraryFolderRec) => void;
}

export function ProjectTree(props: TreeProps) {
  const { project, filter } = props;
  const view = useMemo(
    () =>
      buildLibraryTree(project, filter === "all" ? undefined : (e) => e.kind === filter),
    [project, filter],
  );
  const choices = useMemo(() => folderChoices(project.folders), [project.folders]);
  const byPath = useMemo(
    () => new Map(project.folders.map((f) => [f.path, f])),
    [project.folders],
  );
  // AI folders start open: the marker is only useful if the user sees it.
  const [open, setOpen] = useState<Set<string>>(
    () => new Set(awaitingFolders(project).map((f) => f.path)),
  );
  const toggle = (path: string) =>
    setOpen((prev) => {
      const next = new Set(prev);
      if (next.has(path)) next.delete(path);
      else next.add(path);
      return next;
    });

  const renderFolder = (node: LibraryFolderNode, depth: number): ReactNode => {
    const rec = byPath.get(node.path);
    const review = folderAwaitingReview(node.origin);
    const isOpen = open.has(node.path);
    return (
      <div key={node.path} className={styles.folderBlock}>
        <div className={styles.folderRow} style={{ paddingLeft: 10 + depth * 16 }}>
          <button
            type="button"
            className={styles.folderMain}
            onClick={() => toggle(node.path)}
            aria-expanded={isOpen}
            data-folder={node.path}
          >
            <ChevronRight size={14} className={styles.caret} data-open={isOpen} />
            {isOpen ? (
              <FolderOpen size={16} strokeWidth={1.5} className={styles.folderIcon} />
            ) : (
              <Folder size={16} strokeWidth={1.5} className={styles.folderIcon} />
            )}
            <span className={styles.folderLabel}>{node.label}</span>
            {review && (
              <span className={styles.aiBadge} title={node.origin?.reason}>
                <Sparkles size={10} strokeWidth={1.8} />
                {t("AI 新建")}
              </span>
            )}
            <span className={styles.count}>{node.total}</span>
          </button>
          {rec && (
            <button
              type="button"
              className={styles.more}
              onClick={() => props.onFolderMenu(rec)}
              aria-label={t("目录操作")}
            >
              <MoreHorizontal size={16} />
            </button>
          )}
        </div>
        {isOpen && (
          <>
            {node.children.map((c) => renderFolder(c, depth + 1))}
            {node.items.map((e) => (
              <EntryRow key={libraryEntryKey(e)} entry={e} depth={depth + 1} choices={choices} {...props} />
            ))}
            {node.total === 0 && (
              <div className={styles.empty} style={{ paddingLeft: 34 + (depth + 1) * 16 }}>
                {t("空目录")}
              </div>
            )}
          </>
        )}
      </div>
    );
  };

  return (
    <div className={styles.tree} data-library-tree>
      {view.unfiled.length > 0 && (
        <div className={styles.folderBlock}>
          <div className={styles.folderRow} style={{ paddingLeft: 10 }}>
            <button
              type="button"
              className={styles.folderMain}
              onClick={() => toggle("\u0000unfiled")}
              aria-expanded={!open.has("\u0000unfiled")}
            >
              <ChevronRight
                size={14}
                className={styles.caret}
                data-open={!open.has("\u0000unfiled")}
              />
              <Inbox size={16} strokeWidth={1.5} className={styles.folderIcon} />
              <span className={styles.folderLabel}>{t("未归档")}</span>
              <span className={styles.count}>{view.unfiled.length}</span>
            </button>
          </div>
          {/* The unfiled bucket starts open (its key toggles it *closed*):
              it is what still needs filing. */}
          {!open.has("\u0000unfiled") &&
            view.unfiled.map((e) => (
              <EntryRow key={libraryEntryKey(e)} entry={e} depth={1} choices={choices} {...props} />
            ))}
        </div>
      )}
      {view.folders.map((n) => renderFolder(n, 0))}
      {view.folders.length === 0 && view.unfiled.length === 0 && (
        <div className={styles.empty} style={{ paddingLeft: 14 }}>
          {t("这个项目还没有资料")}
        </div>
      )}
    </div>
  );
}

function EntryRow({
  entry,
  depth,
  choices,
  docs,
  artifacts,
  busy,
  onOpenEntry,
  onMoveEntry,
}: TreeProps & {
  entry: LibraryEntry;
  depth: number;
  choices: Array<{ path: string; label: string }>;
}) {
  const artifact = entry.kind === "artifact" ? artifacts.get(entry.id) : undefined;
  const doc = entry.kind === "doc" ? docs.get(entry.id) : undefined;
  const Icon = entry.kind === "artifact" ? (KIND_ICON[entry.artifactKind] ?? FileText) : null;
  const meta = [
    entry.kind === "doc" ? t("文档") : t("产出"),
    artifact ? formatBytes(artifact.sizeBytes) : "",
    fmtDate(entry.updatedMs),
  ].filter(Boolean);
  const summary = artifact?.summary || doc?.summary || "";
  const moving = busy.has(libraryEntryKey(entry));
  return (
    <div className={styles.entryRow} style={{ paddingLeft: 12 + depth * 16 }} data-entry={entry.id}>
      <button type="button" className={styles.entryMain} onClick={() => onOpenEntry(entry)}>
        {Icon ? (
          <span className={styles.entryIcon}>
            <Icon size={16} strokeWidth={1.5} />
          </span>
        ) : (
          <span className={`${styles.entryIcon} ${styles.docIcon}`}>MD</span>
        )}
        <span className={listStyles.body}>
          <span className={listStyles.title}>{entry.title}</span>
          <span className={listStyles.line2}>{summary || meta.join(" · ")}</span>
        </span>
      </button>
      {/* A native select under the icon: tapping it opens the system picker
          (the wheel on iOS) — no custom sheet to dismiss. */}
      <label className={styles.moveBtn} aria-label={t("移到…")}>
        {moving ? <Spinner size={14} /> : <FolderInput size={16} strokeWidth={1.6} />}
        <select
          className={styles.moveSelect}
          value={entry.folder ?? ""}
          disabled={moving}
          onChange={(e) => onMoveEntry(entry, e.target.value)}
        >
          <option value="">{t("未归档")}</option>
          {choices.map((c) => (
            <option key={c.path} value={c.path}>
              {c.label}
            </option>
          ))}
        </select>
      </label>
    </div>
  );
}

// ── Folder sheet: keep / rename / delete ────────────────────────────────────

export function FolderSheet({
  client,
  folder,
  onClose,
  onChanged,
}: {
  client: FleetTransport | null;
  folder: LibraryFolderRec;
  onClose: () => void;
  /** Something changed: reload the tree. */
  onChanged: () => void;
}) {
  const confirm = useConfirm();
  const [name, setName] = useState(folder.path);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const review = folderAwaitingReview(folder.origin);

  const run = async (key: string, op: () => Promise<unknown>) => {
    if (!client) return;
    setBusy(key);
    setError(null);
    try {
      await op();
      onChanged();
      onClose();
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(null);
    }
  };
  const target = name.trim().replace(/^\/+|\/+$/g, "");

  return (
    <div className={styles.backdrop} onClick={onClose}>
      <div className={styles.sheet} onClick={(e) => e.stopPropagation()} data-folder-sheet>
        <div className={styles.sheetHead}>
          <span className={styles.sheetTitle}>
            <FolderOpen size={16} strokeWidth={1.6} />
            {folder.path}
          </span>
          <button type="button" className={styles.close} onClick={onClose} aria-label={t("关闭")}>
            <X size={18} />
          </button>
        </div>
        {folder.description && <div className={styles.sheetText}>{folder.description}</div>}
        {review && folder.origin && (
          <div className={styles.notice}>
            <Sparkles size={13} strokeWidth={1.6} />
            <span>
              {folder.origin.source === "upload" ? t("上传整理时 AI 新建") : t("Agent 归档时新建")}
              {folder.origin.reason ? t("：{0}", folder.origin.reason) : ""}
            </span>
          </div>
        )}
        {review && (
          <button
            type="button"
            className={styles.primary}
            disabled={busy !== null}
            onClick={() => void run("keep", () => acceptFolder(client!, folder.workspacePath, folder.path))}
          >
            {busy === "keep" && <Spinner size={13} />}
            {t("保留这个目录")}
          </button>
        )}
        <div className={styles.renameRow}>
          <input
            className={styles.input}
            value={name}
            onChange={(e) => setName(e.target.value)}
            aria-label={t("目录名")}
            enterKeyHint="done"
          />
          <button
            type="button"
            className={styles.secondary}
            disabled={busy !== null || !target || target === folder.path}
            onClick={() =>
              void run("rename", () => renameFolder(client!, folder.workspacePath, folder.path, target))
            }
          >
            {busy === "rename" && <Spinner size={13} />}
            {t("改名")}
          </button>
        </div>
        <div className={styles.sheetHint}>{t("用 / 分层，比如「发票/2026」。")}</div>
        <button
          type="button"
          className={styles.danger}
          disabled={busy !== null}
          onClick={() =>
            void (async () => {
              if (!(await confirm(t("删除目录「{0}」？目录里还有资料时不能删。", folder.path)))) return;
              await run("delete", () => deleteFolder(client!, folder.workspacePath, folder.path));
            })()
          }
        >
          {busy === "delete" && <Spinner size={13} />}
          {t("删除目录")}
        </button>
        {error && <div className={styles.error}>{error}</div>}
      </div>
    </div>
  );
}

function fmtDate(ms: number): string {
  if (!ms) return "";
  return new Date(ms).toLocaleDateString(dateLocale(), { month: "2-digit", day: "2-digit" });
}
