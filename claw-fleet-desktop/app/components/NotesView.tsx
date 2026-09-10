import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { TextBlock } from "./blocks/TextBlock";
import { useResizableWidth } from "../hooks/useResizableWidth";
import { ResizeHandle } from "./ResizeHandle";
import { formatBytes } from "../formatBytes";
import type { NoteFile, NoteMatch } from "../types";
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
  const [query, setQuery] = useState("");
  const [matches, setMatches] = useState<NoteMatch[] | null>(null);
  /* Only identity is needed to read a note back, and a search hit carries just
     that (no bytes / mtime) — so the selection is the pair, not a NoteFile. */
  const [active, setActive] = useState<{ sessionId: string; path: string } | null>(null);
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

  /* Search is literal and case-sensitive (core's `session_notes::search`), so
     it is cheap enough to run per keystroke behind a short debounce rather than
     needing a submit button. An empty box means "not searching" — `null`, not
     an empty result list, so the tree shows the file list again instead of
     "no hits". */
  useEffect(() => {
    const q = query.trim();
    if (!q) {
      setMatches(null);
      return;
    }
    let cancelled = false;
    const timer = setTimeout(() => {
      invoke<NoteMatch[]>("search_session_notes", { sessionId, query: q })
        .then((hits) => {
          if (!cancelled) setMatches(hits ?? []);
        })
        .catch(() => {
          if (!cancelled) setMatches([]);
        });
    }, 250);
    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [sessionId, query]);

  const pick = useCallback(
    (file: { sessionId: string; path: string }) => {
      setActive({ sessionId: file.sessionId, path: file.path });
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
        {files.length > 0 && (
          <input
            className={skillStyles.tree_filter}
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder={t("detail.notes_search_placeholder")}
            spellCheck={false}
          />
        )}

        {files.length === 0 ? (
          <div className={skillStyles.tree_empty}>{t("detail.notes_empty")}</div>
        ) : matches != null ? (
          /* Searching: the tree becomes the hit list. Each row is one matched
             line, so clicking it opens that note with the term highlighted —
             the file list is one keystroke away (clear the box) and repeating
             it here would just push the hits off screen. */
          matches.length === 0 ? (
            <div className={skillStyles.tree_empty}>{t("detail.notes_search_none")}</div>
          ) : (
            matches.map((m) => (
              <button
                key={`${m.sessionId}:${m.path}:${m.line}`}
                className={`${skillStyles.tree_item} ${
                  active?.sessionId === m.sessionId && active?.path === m.path
                    ? skillStyles.tree_item_active
                    : ""
                }`}
                onClick={() => pick(m)}
                title={`${m.path}:${m.line}\n${m.text}`}
              >
                <span className={skillStyles.tree_name}>{m.text.trim() || m.path}</span>
                <span className={skillStyles.tree_size}>
                  {m.sessionId === sessionId ? `:${m.line}` : `${m.sessionId.slice(0, 4)}:${m.line}`}
                </span>
              </button>
            ))
          )
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
          /* The same term the hit list matched on, so the reader lands on a
             page where the line they clicked is already marked. */
          <TextBlock text={content} searchTerms={query.trim() ? [query.trim()] : undefined} />
        )}
      </div>
    </div>
  );
}
