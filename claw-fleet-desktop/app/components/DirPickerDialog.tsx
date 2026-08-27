import { useCallback, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { ChevronUp, Folder, FolderGit2, FolderPlus } from "lucide-react";
import styles from "./DirPickerDialog.module.css";

/** Mirrors `claw_fleet_core::workspace_browse::BrowseDirResponse`. */
interface BrowseEntry {
  name: string;
  path: string;
  isGitRepo: boolean;
}
interface BrowseDirResponse {
  path: string;
  parent: string | null;
  entries: BrowseEntry[];
  truncated: boolean;
}

interface Props {
  /** Where to open; empty starts at the backend host's home. */
  initialPath: string;
  onPick: (path: string) => void;
  onCancel: () => void;
}

/** Pick a workspace directory **on the backend host**.
 *
 *  Tauri's native directory dialog can only browse the machine the desktop runs
 *  on. Under a remote connection that is the wrong host — the session will be
 *  spawned on the probe — so the launcher used to just hide its Browse button
 *  and leave remote users unable to choose any directory that had never had a
 *  session. This dialog goes through `browse_dir` on the Backend trait instead,
 *  so it lists whichever host the backend is bound to. */
export function DirPickerDialog({ initialPath, onPick, onCancel }: Props) {
  const { t } = useTranslation();
  const [data, setData] = useState<BrowseDirResponse | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async (path?: string, fallbackToHome = false) => {
    setLoading(true);
    setError(null);
    try {
      setData(await invoke<BrowseDirResponse>("browse_dir", { path: path ?? null }));
    } catch (e) {
      setError(String(e));
      // A dead starting point (a workspace that has since been deleted, or —
      // under a remote connection — a path that only exists on the desktop)
      // would otherwise strand the user: the listing is empty, so there is not
      // even a row to click away from. Fall back to the host's home so the
      // picker is always navigable; the error stays on screen to explain why.
      if (fallbackToHome && path) {
        try {
          setData(await invoke<BrowseDirResponse>("browse_dir", { path: null }));
        } catch {
          // Home itself is unreachable — nothing left to fall back to.
        }
      }
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    void load(initialPath.trim() || undefined, true);
  }, [load, initialPath]);

  // Make a directory to pick. A picker that can only walk down an existing tree
  // has no answer at all on a host whose tree is empty — a freshly provisioned
  // one always is. `create_dir` replies with the new directory's own listing, so
  // one round trip both creates it and steps into it.
  const [creating, setCreating] = useState(false);
  const [newName, setNewName] = useState("");
  const [saving, setSaving] = useState(false);

  const submitNew = async () => {
    const name = newName.trim();
    if (!data || !name || saving) return;
    setSaving(true);
    setError(null);
    try {
      setData(await invoke<BrowseDirResponse>("create_dir", { path: data.path, name }));
      setNewName("");
      setCreating(false);
    } catch (e) {
      // Name taken, no write permission — stay in the input so a retype retries.
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  // Long paths matter at their tail (which directory am I in), not their head.
  const crumbRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = crumbRef.current;
    if (el) el.scrollLeft = el.scrollWidth;
  }, [data?.path]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  return (
    <div className={styles.overlay} onClick={onCancel}>
      <div className={styles.dialog} onClick={(e) => e.stopPropagation()}>
        <div className={styles.title}>{t("dir_picker.title")}</div>
        <div className={styles.crumb} ref={crumbRef}>
          {data?.path ?? initialPath ?? "…"}
        </div>

        {error && <div className={styles.error}>{error}</div>}

        <div className={styles.list}>
          {data?.parent && (
            <button className={styles.row} onClick={() => void load(data.parent!)}>
              <ChevronUp size={13} strokeWidth={1.7} className={styles.icon} />
              <span className={styles.name}>{t("dir_picker.up")}</span>
            </button>
          )}
          {data?.entries.map((e) => (
            <button key={e.path} className={styles.row} onClick={() => void load(e.path)}>
              {e.isGitRepo ? (
                <FolderGit2 size={13} strokeWidth={1.7} className={styles.iconRepo} />
              ) : (
                <Folder size={13} strokeWidth={1.7} className={styles.icon} />
              )}
              <span className={styles.name}>{e.name}</span>
            </button>
          ))}
          {data && !loading && data.entries.length === 0 && (
            <div className={styles.empty}>{t("dir_picker.empty")}</div>
          )}
          {data?.truncated && <div className={styles.empty}>{t("dir_picker.truncated")}</div>}
          {loading && <div className={styles.empty}>{t("dir_picker.loading")}</div>}
        </div>

        {creating ? (
          <div className={styles.newRow}>
            <input
              className={styles.newInput}
              autoFocus
              value={newName}
              placeholder={t("dir_picker.new_name")}
              onChange={(e) => setNewName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") void submitNew();
                // Escape backs out of the input, not the whole dialog — stop it
                // before the window-level handler treats it as a cancel.
                if (e.key === "Escape") {
                  e.stopPropagation();
                  setError(null);
                  setCreating(false);
                }
              }}
            />
            <button
              className={styles.confirm}
              disabled={!newName.trim() || saving}
              onClick={() => void submitNew()}
            >
              {saving ? t("dir_picker.creating") : t("dir_picker.create")}
            </button>
            <button
              className={styles.cancel}
              onClick={() => {
                // Drop the error with the input: "already exists" was about that
                // attempt, and left on screen it reads as a problem with the
                // directory being browsed.
                setError(null);
                setCreating(false);
              }}
            >
              {t("cancel")}
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
            <FolderPlus size={13} strokeWidth={1.7} className={styles.icon} />
            <span>{t("dir_picker.new")}</span>
          </button>
        )}

        <div className={styles.actions}>
          <button className={styles.cancel} onClick={onCancel}>
            {t("cancel")}
          </button>
          <button
            className={styles.confirm}
            disabled={!data}
            onClick={() => data && onPick(data.path)}
          >
            {t("dir_picker.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
