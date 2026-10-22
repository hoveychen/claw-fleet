// The phone's folder review: which folders an AI made that still await the
// user, the folder sheet (keep / rename / delete / add a subfolder) and the
// new-folder sheet. The browsing levels
// themselves live in LibraryBrowse.tsx; LibraryView owns the data and the
// overlays.

import { useState } from "react";
import { ChevronDown, FolderOpen, FolderPlus, Sparkles, X } from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import {
  acceptFolder,
  childFolderPath,
  createFolder,
  deleteFolder,
  folderChoices,
  folderAwaitingReview,
  renameFolder,
  type LibraryFolderRec,
  type LibraryProjectTree,
} from "../libraryApi";
import { useConfirm } from "../confirmDialog";
import { libraryErrorOf, libraryErrorView, type LibraryErrorView } from "../libraryErrors";
import { LibraryErrorNote } from "./LibraryErrorNote";
import { Spinner } from "./loading";
import styles from "./LibraryTree.module.css";

/** Folders an AI made that nobody has kept, renamed or deleted yet. */
export function awaitingFolders(project: Pick<LibraryProjectTree, "folders">): LibraryFolderRec[] {
  return project.folders.filter((f) => folderAwaitingReview(f.origin));
}

// ── Folder sheet: keep / rename / delete ────────────────────────────────────

export function FolderSheet({
  client,
  folder,
  onClose,
  onChanged,
  onNewSubfolder,
}: {
  client: FleetTransport | null;
  folder: LibraryFolderRec;
  onClose: () => void;
  /** Something changed: reload the tree. */
  onChanged: () => void;
  /** Open the new-folder sheet with this folder as the parent. */
  onNewSubfolder?: (parent: string) => void;
}) {
  const confirm = useConfirm();
  const [name, setName] = useState(folder.path);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<LibraryErrorView | null>(null);
  const review = folderAwaitingReview(folder.origin);

  const run = async (key: string, action: string, op: () => Promise<unknown>) => {
    if (!client) return;
    setBusy(key);
    setError(null);
    try {
      await op();
      onChanged();
      onClose();
    } catch (e) {
      setError(libraryErrorOf(e, action));
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
            onClick={() => void run("keep", t("保留目录"), () => acceptFolder(client!, folder.workspacePath, folder.path))}
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
              void run("rename", t("改名"), () => renameFolder(client!, folder.workspacePath, folder.path, target))
            }
          >
            {busy === "rename" && <Spinner size={13} />}
            {t("改名")}
          </button>
        </div>
        <div className={styles.sheetHint}>{t("用 / 分层，比如「发票/2026」。")}</div>
        {onNewSubfolder && (
          <button
            type="button"
            className={styles.secondary}
            disabled={busy !== null}
            onClick={() => onNewSubfolder(folder.path)}
            data-new-subfolder
          >
            <FolderPlus size={15} strokeWidth={1.6} />
            {t("新建子目录")}
          </button>
        )}
        <button
          type="button"
          className={styles.danger}
          disabled={busy !== null}
          onClick={() =>
            void (async () => {
              if (!(await confirm(t("删除目录「{0}」？目录里还有资料时不能删。", folder.path)))) return;
              await run("delete", t("删除目录"), () => deleteFolder(client!, folder.workspacePath, folder.path));
            })()
          }
        >
          {busy === "delete" && <Spinner size={13} />}
          {t("删除目录")}
        </button>
        <LibraryErrorNote error={error} className={styles.error} />
      </div>
    </div>
  );
}

// ── New folder sheet ────────────────────────────────────────────────────────

/**
 * Add a folder to a project, at the top level or under `parent`. The parent
 * stays changeable (a native select, the phone's wheel), and a name may hold
 * `/` to add several levels at once, as rename allows.
 *
 * The host answers an existing path with that folder instead of an error, so a
 * name already taken is caught here, against the tree on screen — otherwise
 * "create" would silently do nothing.
 */
export function NewFolderSheet({
  client,
  project,
  parent: initialParent,
  onClose,
  onCreated,
}: {
  client: FleetTransport | null;
  project: Pick<LibraryProjectTree, "workspacePath" | "workspaceName" | "folders">;
  /** `""` = top level. */
  parent: string;
  onClose: () => void;
  onCreated: (path: string) => void;
}) {
  const [parent, setParent] = useState(initialParent);
  const [name, setName] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<LibraryErrorView | null>(null);
  const choices = folderChoices(project.folders);
  const target = childFolderPath(parent, name);
  const taken = !!name.trim() && project.folders.some((f) => f.path === target);

  const submit = async () => {
    if (!client || busy || !name.trim()) return;
    if (taken) {
      setError(libraryErrorView({ code: "folder-exists", args: [target], raw: "" }));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const made = await createFolder(client, project.workspacePath, target);
      onCreated(made.path);
      onClose();
    } catch (e) {
      setError(libraryErrorOf(e, t("新建目录")));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className={styles.backdrop} onClick={onClose}>
      <div className={styles.sheet} onClick={(e) => e.stopPropagation()} data-new-folder-sheet>
        <div className={styles.sheetHead}>
          <span className={styles.sheetTitle}>
            <FolderPlus size={16} strokeWidth={1.6} />
            {parent ? t("新建子目录") : t("新建目录")}
          </span>
          <button type="button" className={styles.close} onClick={onClose} aria-label={t("关闭")}>
            <X size={18} />
          </button>
        </div>
        <label className={styles.parentWrap}>
          <span className={styles.parentLabel}>{t("放在")}</span>
          <select
            className={styles.parent}
            value={parent}
            disabled={busy}
            onChange={(e) => {
              setParent(e.target.value);
              setError(null);
            }}
            data-new-folder-parent
          >
            <option value="">{t("{0}（顶层）", project.workspaceName)}</option>
            {choices.map((c) => (
              <option key={c.path} value={c.path}>
                {c.label}
              </option>
            ))}
          </select>
          <ChevronDown size={14} aria-hidden="true" />
        </label>
        <form
          className={styles.renameRow}
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          <input
            className={styles.input}
            value={name}
            onChange={(e) => {
              setName(e.target.value);
              setError(null);
            }}
            placeholder={t("目录名")}
            aria-label={t("目录名")}
            enterKeyHint="done"
            autoFocus
            data-new-folder-name
          />
          <button type="submit" className={styles.primary} disabled={busy || !client || !name.trim()}>
            {busy && <Spinner size={13} />}
            {t("新建")}
          </button>
        </form>
        <div className={styles.sheetHint}>
          {target && target !== parent
            ? t("将新建「{0}」。用 / 可一次建多层。", target)
            : t("用 / 可一次建多层，比如「发票/2026」。")}
        </div>
        <LibraryErrorNote error={error} className={styles.error} />
      </div>
    </div>
  );
}
