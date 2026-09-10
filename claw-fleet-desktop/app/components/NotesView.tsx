import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { TextBlock } from "./blocks/TextBlock";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { ResizeHandle } from "./ResizeHandle";
import { formatBytes } from "../formatBytes";
import type { NoteFile } from "../types";
import styles from "./MemoryView.module.css";
import skillStyles from "./SkillsView.module.css";

/**
 * Read-only browser for the session's checkpoint notes (`~/.fleet/notes/`).
 *
 * These are what the agent writes to survive a context compaction — the store
 * behind the `fleet__notes` tool. Until this panel existed they were reachable
 * only from a terminal (`fleet notes read …`), so a reader watching a long run
 * could see the "追加笔记" tool card scroll past and still have no way to open
 * the file it had just appended to.
 *
 * The list spans the session *and its handoff predecessors*, matching what the
 * agent itself can read: on a 68-hop relay chain the useful checkpoint was
 * usually taken by an earlier hop. Each row therefore carries its own owner,
 * and reads go by that owner rather than being re-resolved — the whole chain
 * tends to keep a file called `checkpoint.md`.
 */
export function NotesView({ sessionId }: { sessionId: string }) {
  const { t } = useTranslation();
  const [files, setFiles] = useState<NoteFile[]>([]);
  const [active, setActive] = useState<NoteFile | null>(null);
  const [content, setContent] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const {
    width: treeWidth,
    isDragging: treeDragging,
    onMouseDown: onTreeMouseDown,
  } = useResizableWidth("notes-tree-width", { min: 140, max: 480, initial: 240 });

  useEffect(() => {
    let cancelled = false;
    invoke<NoteFile[]>("list_session_notes", { sessionId })
      .then((list) => {
        if (!cancelled) setFiles(list ?? []);
      })
      .catch(() => {
        if (!cancelled) setFiles([]);
      });
    return () => {
      cancelled = true;
    };
  }, [sessionId]);

  const pick = useCallback(
    (file: NoteFile) => {
      setActive(file);
      setContent(null);
      setError(null);
      // The owner, not the session on screen: an inherited `checkpoint.md`
      // must read back as the predecessor wrote it.
      invoke<string>("read_session_note", {
        sessionId: file.sessionId,
        path: file.path,
      })
        .then(setContent)
        .catch((e) => setError(String(e)));
    },
    [],
  );

  /* Own notes first, then one group per predecessor — the same order the agent
     sees them in, and the order that puts "what this run recorded" on top. */
  const groups = useMemo(() => {
    const byOwner = new Map<string, NoteFile[]>();
    for (const f of files) {
      const bucket = byOwner.get(f.sessionId);
      if (bucket) bucket.push(f);
      else byOwner.set(f.sessionId, [f]);
    }
    return [...byOwner.entries()].map(([owner, items]) => ({
      owner,
      isOwn: owner === sessionId,
      items,
    }));
  }, [files, sessionId]);

  return (
    <div className={skillStyles.detail_split}>
      <aside className={skillStyles.tree_pane} style={{ width: treeWidth }}>
        {files.length === 0 ? (
          <div className={skillStyles.tree_empty}>{t("detail.notes_empty")}</div>
        ) : (
          groups.map((g) => (
            <div key={g.owner}>
              <div className={skillStyles.tree_label} title={g.owner}>
                {g.isOwn
                  ? t("detail.notes_owner_self")
                  : t("detail.notes_owner_predecessor", { id: g.owner.slice(0, 8) })}
              </div>
              {g.items.map((f) => (
                <button
                  key={`${f.sessionId}:${f.path}`}
                  className={`${skillStyles.tree_item} ${
                    active?.sessionId === f.sessionId && active?.path === f.path
                      ? skillStyles.tree_item_active
                      : ""
                  }`}
                  onClick={() => pick(f)}
                  title={f.path}
                >
                  <span className={skillStyles.tree_name}>{f.path}</span>
                  <span className={skillStyles.tree_size}>{formatBytes(f.bytes)}</span>
                </button>
              ))}
            </div>
          ))
        )}

        <ResizeHandle active={treeDragging} onMouseDown={onTreeMouseDown} />
      </aside>

      <div className={styles.detail_body}>
        {!active ? (
          <p className={styles.empty}>{t("detail.notes_hint")}</p>
        ) : error ? (
          <p className={styles.empty}>{error}</p>
        ) : content == null ? (
          <p className={styles.empty}>{t("detail.notes_loading")}</p>
        ) : (
          <TextBlock text={content} />
        )}
      </div>
    </div>
  );
}
