import { useCallback, useEffect, useRef, useState } from "react";
import { ChevronUp, Folder, FolderGit2, FolderPlus, HardDrive, X } from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import type { BrowseDirResponse } from "../types";
import styles from "./DirPicker.module.css";

interface DirPickerProps {
  client: FleetTransport | null;
  /** Starting directory; empty string begins from desktop home. */
  initialPath: string;
  onPick: (path: string) => void;
  onClose: () => void;
}

/** Pick a directory on the desktop machine as workspace on mobile.
 *
 *  Replaces the original bare input box—mobile users cannot see which directories
 *  exist on the desktop, forcing them to type absolute paths blindly, which is poor UX.
 *  Desktop's `browse_dir` returns only one level of subdirectories each time, and it decides
 *  whether we can go up (parent is null at the browsable boundary), so no path joining here. */
export function DirPicker({ client, initialPath, onPick, onClose }: DirPickerProps) {
  const [data, setData] = useState<BrowseDirResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(
    async (path?: string, fallbackToHome = false) => {
      if (!client) return;
      setLoading(true);
      setError(null);
      try {
        setData(await client.request<BrowseDirResponse>("browse_dir", path ? { path } : {}));
      } catch (e) {
        // Keep the previous screen on failure so user can go back; avoid blank page.
        setError(e instanceof Error ? e.message : t("读取目录失败"));
        // But when the initial path fails, there is no previous screen—the user may have typed
        // a path that was deleted, the list is empty with no clickable rows, and they'd have to
        // close and retry. Fall back to home to ensure the picker always works; error stays to explain.

        if (fallbackToHome && path) {
          try {
            setData(await client.request<BrowseDirResponse>("browse_dir", {}));
          } catch {
            // Home is also unreadable; no further fallback.
          }
        }
      } finally {
        setLoading(false);
      }
    },
    [client],
  );

  useEffect(() => {
    void load(initialPath || undefined, true);
  }, [load, initialPath]);

  // Create subdirectory. A downward-only picker has no options when the directory tree is empty—
  // a new cloud container has nothing under `/home/fleet`, list shows only "no subdirectories",
  // making the picker unusable. Desktop's `create_dir` returns the listing of the new directory,
  // so one round trip puts us inside it, then we can click "use this directory".
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
  const [saving, setSaving] = useState(false);

  const submitNew = async () => {
    const name = newName.trim();
    if (!client || !data || !name || saving) return;
    setSaving(true);
    setError(null);
    try {
      setData(
        await client.request<BrowseDirResponse>("create_dir", { path: data.path, name }),
      );
      setNewName("");
      setCreating(false);
    } catch (e) {
      // Duplicate name or no permission—stay in input mode, user can retry with a different name.
      setError(e instanceof Error ? e.message : t("新建目录失败"));
    } finally {
      setSaving(false);
    }
  };

  // When path is longer than screen, the meaningful part is the tail (current directory), not the `/Users` prefix.
  const crumbRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = crumbRef.current;
    if (el) el.scrollLeft = el.scrollWidth;
  }, [data?.path]);

  return (
    <div className={styles.backdrop} onClick={onClose}>
      <div className={styles.sheet} onClick={(e) => e.stopPropagation()}>
        <div className={styles.head}>
          <span className={styles.title}>{t("选择工作目录")}</span>
          <button className={styles.close} onClick={onClose} aria-label={t("关闭")}>
            <X size={18} />
          </button>
        </div>

        <div className={styles.crumb} ref={crumbRef}>
          {data?.path ?? initialPath ?? "…"}
        </div>

        {error && <div className={styles.error}>{error}</div>}

        {creating ? (
          <div className={styles.newRow}>
            <input
              className={styles.newInput}
              autoFocus
              value={newName}
              placeholder={t("新目录名")}
              onChange={(e) => setNewName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void submitNew();
                if (e.key === "Escape") {
                  setError(null);
                  setCreating(false);
                }
              }}
            />
            <button
              className={styles.newOk}
              disabled={!newName.trim() || saving}
              onClick={() => void submitNew()}
            >
              {saving ? t("创建中…") : t("创建")}
            </button>
            <button
              className={styles.newCancel}
              onClick={() => {
                // Clear the error when leaving the input state — "already exists"
                // described that one attempt, and leaving it on screen would read as
                // if the current directory itself were broken.
                setError(null);
                setCreating(false);
              }}
            >
              {t("取消")}
            </button>
          </div>
        ) : (
          <button
            className={styles.newBtn}
            disabled={!data}
            onClick={() => {
              setNewName("");
              setError(null);
              setCreating(true);
            }}
          >
            <FolderPlus size={16} className={styles.icon} />
            <span>{t("在这里新建子目录")}</span>
          </button>
        )}

        <div className={styles.list}>
          {/* When standing in a root, there is no "parent" to click—roots don't expose their parent.
              Cloud container's starting point is such a root (persistent volume), home is on another,
              so we list other roots as clickable rows; otherwise users can only switch roots by typing. */}
          {!data?.parent &&
            (data?.roots ?? [])
              .filter((r) => r !== data?.path)
              .map((r) => (
                <button key={r} className={styles.row} onClick={() => void load(r)}>
                  <HardDrive size={16} className={styles.icon} />
                  {/* Directory name first: full path in one line gets truncated from the tail,
                      which is exactly the part that identifies it (`/private/tmp/claude-501/-Users-…` says nothing).
                      Full path follows after; even if truncated, identification is unaffected. */}
                  <span className={styles.name}>{r.split("/").filter(Boolean).pop() ?? r}</span>
                  <span className={styles.rowPath}>{r}</span>
                </button>
              ))}
          {data?.parent && (
            <button className={styles.row} onClick={() => void load(data.parent!)}>
              <ChevronUp size={16} className={styles.icon} />
              <span className={styles.name}>{t("上一级")}</span>
            </button>
          )}
          {data?.entries.map((e) => (
            <button key={e.path} className={styles.row} onClick={() => void load(e.path)}>
              {e.isGitRepo ? (
                <FolderGit2 size={16} className={styles.iconRepo} />
              ) : (
                <Folder size={16} className={styles.icon} />
              )}
              <span className={styles.name}>{e.name}</span>
            </button>
          ))}
          {data && !loading && data.entries.length === 0 && (
            <div className={styles.empty}>{t("这里没有子目录")}</div>
          )}
          {data?.truncated && (
            <div className={styles.empty}>{t("子目录过多，仅显示前 500 个")}</div>
          )}
          {loading && <div className={styles.empty}>{t("读取中…")}</div>}
        </div>

        <button
          className={styles.confirm}
          disabled={!data}
          onClick={() => data && onPick(data.path)}
        >
          {t("用这个目录")}
        </button>
      </div>
    </div>
  );
}
