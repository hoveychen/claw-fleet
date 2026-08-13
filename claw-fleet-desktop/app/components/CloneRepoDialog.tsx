import { invoke } from "@tauri-apps/api/core";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { FolderOpen } from "lucide-react";
import { DirPickerDialog } from "./DirPickerDialog";
import { ProcTerminal } from "./ProcTerminal";
import type { ProcRecord } from "../types";
import styles from "./CloneRepoDialog.module.css";

/** Directory name `git clone` itself would pick for `url`: the last path
 *  segment with any `.git` suffix dropped. Handles both URL forms git accepts —
 *  `https://host/owner/repo.git` and scp-style `git@host:owner/repo.git` — plus
 *  trailing slashes. Empty when nothing usable can be derived, which is the
 *  signal to keep the Clone button disabled. */
export function repoDirNameFromUrl(url: string): string {
  const trimmed = url.trim().replace(/[/\\]+$/, "");
  if (!trimmed) return "";
  // scp-style URLs have no `//`, so the colon is the host/path boundary; for
  // real URLs a `/` always follows it. Splitting on both covers each form.
  const seg = trimmed.split(/[/\\:]/).pop() ?? "";
  return seg.replace(/\.git$/i, "");
}

/** `parent` + `name` as one path, keeping the separator the parent already uses
 *  so a Windows path doesn't come back half-slashed. */
export function joinPath(parent: string, name: string): string {
  const sep = parent.includes("\\") && !parent.includes("/") ? "\\" : "/";
  return `${parent.replace(/[/\\]+$/, "")}${sep}${name}`;
}

interface Props {
  /** Pre-filled target directory (the selected workspace's parent, usually). */
  initialParent: string;
  /** Remote connection → browse the probe host via the backend picker, since
   *  the native dialog would browse *this* desktop (mirrors FilesView). */
  isRemote: boolean;
  /** Clone succeeded; `dest` is the new checkout's absolute path. */
  onDone: (dest: string) => void;
  onCancel: () => void;
}

/** Clone a git repository into a directory the user picks.
 *
 *  The clone runs on whichever host the backend is bound to (local desktop or
 *  the remote probe) as a *streaming* workspace command, so git's own progress
 *  counters ("Receiving objects: 47% …") land in a terminal here rather than
 *  the dialog sitting on a spinner for the whole transfer. That reuses the
 *  existing proc runner wholesale — same detached pty host, same incremental
 *  output polling, already at parity between Local and Remote backends. */
export function CloneRepoDialog({ initialParent, isRemote, onDone, onCancel }: Props) {
  const { t } = useTranslation();
  const [url, setUrl] = useState("");
  const [parent, setParent] = useState(initialParent);
  const [name, setName] = useState("");
  const [picking, setPicking] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // The running clone. Set once it's spawned; the terminal below tails it.
  const [proc, setProc] = useState<ProcRecord | null>(null);

  // The name field is a *override* of the derived default, so an untouched
  // field keeps tracking whatever the user pastes into the URL box.
  const dirName = name.trim() || repoDirNameFromUrl(url);
  const dest = parent.trim() && dirName ? joinPath(parent.trim(), dirName) : "";
  const ready = url.trim().length > 0 && dest.length > 0 && !busy;

  const browse = async () => {
    if (isRemote) {
      setPicking(true);
      return;
    }
    const picked = await openDialog({ multiple: false, directory: true });
    if (typeof picked === "string") setParent(picked);
  };

  const submit = async () => {
    if (!ready) return;
    setBusy(true);
    setError(null);
    try {
      // Returns as soon as the proc is spawned — the guard rails (absolute
      // dest, existing parent, empty destination, url shape) are enforced
      // before that, so a rejection still surfaces here rather than in the
      // terminal.
      setProc(await invoke<ProcRecord>("start_git_clone", { url: url.trim(), dest }));
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  };

  /// Every output poll hands back the proc's record; act on the first one that
  /// reports an exit. `onDone` is what registers the new checkout as browsable.
  const onProcRecord = (record: ProcRecord) => {
    if (record.status !== "exited" || !busy) return;
    setBusy(false);
    if (record.exitCode === 0) onDone(dest);
    else setError(t("files.clone.failed"));
  };

  // Escape closes — but never mid-clone, when the dialog is the only place the
  // eventual git output can land.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape" && !busy) onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel, busy]);

  if (picking) {
    return (
      <DirPickerDialog
        initialPath={parent}
        onPick={(path) => {
          setParent(path);
          setPicking(false);
        }}
        onCancel={() => setPicking(false)}
      />
    );
  }

  return (
    <div className={styles.overlay} onClick={() => !busy && onCancel()}>
      <div className={styles.dialog} onClick={(e) => e.stopPropagation()}>
        <div className={styles.title}>{t("files.clone.title")}</div>

        <label className={styles.label} htmlFor="clone-url">
          {t("files.clone.url_label")}
        </label>
        <input
          id="clone-url"
          className={styles.input}
          value={url}
          autoFocus
          spellCheck={false}
          placeholder={t("files.clone.url_placeholder")}
          disabled={busy}
          onChange={(e) => setUrl(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void submit();
          }}
        />

        <label className={styles.label} htmlFor="clone-parent">
          {t("files.clone.parent_label")}
        </label>
        <div className={styles.row}>
          <input
            id="clone-parent"
            className={styles.input}
            value={parent}
            spellCheck={false}
            placeholder={t("files.clone.parent_placeholder")}
            disabled={busy}
            onChange={(e) => setParent(e.target.value)}
          />
          <button className={styles.browse} onClick={() => void browse()} disabled={busy}>
            <FolderOpen size={13} strokeWidth={1.7} />
            {t("files.clone.browse")}
          </button>
        </div>

        <label className={styles.label} htmlFor="clone-name">
          {t("files.clone.name_label")}
        </label>
        <input
          id="clone-name"
          className={styles.input}
          value={name}
          spellCheck={false}
          placeholder={repoDirNameFromUrl(url) || t("files.clone.name_placeholder")}
          disabled={busy}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void submit();
          }}
        />

        {dest && <div className={styles.dest}>{dest}</div>}
        <p className={styles.hint}>{t("files.clone.hint")}</p>

        {/* git's own progress, live. Kept mounted after a failure so the error
            output stays readable while the user edits the url and retries. */}
        {proc && (
          <ProcTerminal
            key={proc.id}
            proc={proc}
            onRecord={onProcRecord}
            height={180}
          />
        )}

        {error && <div className={styles.error}>{error}</div>}

        <div className={styles.actions}>
          <button className={styles.cancel} onClick={onCancel} disabled={busy}>
            {t("cancel")}
          </button>
          <button className={styles.confirm} onClick={() => void submit()} disabled={!ready}>
            {busy ? t("files.clone.cloning") : t("files.clone.submit")}
          </button>
        </div>
      </div>
    </div>
  );
}
