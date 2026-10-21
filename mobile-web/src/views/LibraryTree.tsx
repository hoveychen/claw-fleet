// The phone's folder review: which folders an AI made that still await the
// user, and the folder sheet (keep / rename / delete). The browsing levels
// themselves live in LibraryBrowse.tsx; LibraryView owns the data and the
// overlays.

import { useState } from "react";
import { FolderOpen, Sparkles, X } from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import {
  acceptFolder,
  deleteFolder,
  folderAwaitingReview,
  renameFolder,
  type LibraryFolderRec,
  type LibraryProjectTree,
} from "../libraryApi";
import { useConfirm } from "../confirmDialog";
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
