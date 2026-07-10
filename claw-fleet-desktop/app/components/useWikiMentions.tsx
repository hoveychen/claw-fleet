// `@`-mention autocomplete over the wiki, shared by every prompt box.
//
// Picking a doc inserts a `[[slug]]` reference rather than a path: the slug is
// the doc's stable address (it survives version bumps and the wiki's on-disk
// escaping), and `fleet wiki cat <slug>` is what the agent runs to read it.

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import type { WikiDoc } from "./WikiView";
import styles from "./useWikiMentions.module.css";

/** Longest `@…` run still treated as a mention query rather than prose. */
const MAX_MENTION_QUERY = 48;
const MAX_MENTION_RESULTS = 8;

/**
 * Find the `@query` the caret currently sits inside, if any. A mention starts
 * at an `@` that follows whitespace (or the very start of the text) and runs
 * up to the caret without crossing whitespace.
 */
export function detectMention(
  text: string,
  caret: number,
): { start: number; query: string } | null {
  for (let i = caret - 1; i >= 0; i--) {
    const ch = text[i];
    if (/\s/.test(ch)) return null;
    if (ch !== "@") continue;
    const before = i > 0 ? text[i - 1] : "";
    if (before !== "" && !/\s/.test(before)) return null;
    const query = text.slice(i + 1, caret);
    if (query.length > MAX_MENTION_QUERY) return null;
    return { start: i, query };
  }
  return null;
}

/** Rank docs whose slug or title contains the query; slug matches come first. */
export function filterWikiDocs(docs: WikiDoc[], query: string): WikiDoc[] {
  const q = query.trim().toLowerCase();
  if (!q) return docs.slice(0, MAX_MENTION_RESULTS);
  const bySlug: WikiDoc[] = [];
  const byTitle: WikiDoc[] = [];
  for (const d of docs) {
    if (d.slug.toLowerCase().includes(q)) bySlug.push(d);
    else if (d.title.toLowerCase().includes(q)) byTitle.push(d);
  }
  return [...bySlug, ...byTitle].slice(0, MAX_MENTION_RESULTS);
}

export interface WikiMentions {
  /** The picker, to render inside a `position: relative` container. */
  menu: React.ReactNode;
  /** True while the picker is showing and owns the arrow/Enter/Tab/Esc keys. */
  open: boolean;
  /** Call from `onChange` and `onSelect` to track the caret's `@…` run. */
  sync: (el: HTMLTextAreaElement) => void;
  /** Close the picker (e.g. from `onBlur`). */
  close: () => void;
  /**
   * Call first from `onKeyDown`. Returns true when the picker consumed the
   * key, in which case the host must not also submit or insert a newline.
   */
  handleKeyDown: (e: React.KeyboardEvent) => boolean;
}

/**
 * @param enabled  Off by default so composers opt in.
 * @param value    Current textarea text.
 * @param onChange Called with the text after a doc is inserted.
 */
export function useWikiMentions(
  enabled: boolean,
  value: string,
  onChange: (next: string) => void,
  textareaRef: RefObject<HTMLTextAreaElement | null>,
): WikiMentions {
  const [mention, setMention] = useState<{ start: number; query: string } | null>(null);
  const [docs, setDocs] = useState<WikiDoc[] | null>(null);
  const [activeIdx, setActiveIdx] = useState(0);
  const wrapRef = useRef<HTMLDivElement | null>(null);

  // Load the doc list once, the first time an `@` actually opens the picker.
  useEffect(() => {
    if (!enabled || !mention || docs !== null) return;
    let cancelled = false;
    invoke<WikiDoc[]>("list_wiki_docs")
      .then((d) => {
        if (!cancelled) setDocs(d);
      })
      .catch(() => {
        if (!cancelled) setDocs([]);
      });
    return () => {
      cancelled = true;
    };
  }, [enabled, mention, docs]);

  const matches = mention && docs ? filterWikiDocs(docs, mention.query) : [];
  const open = Boolean(mention) && matches.length > 0;

  useEffect(() => {
    setActiveIdx(0);
  }, [mention?.query, mention?.start]);

  const close = useCallback(() => setMention(null), []);

  const sync = useCallback(
    (el: HTMLTextAreaElement) => {
      if (!enabled) return;
      setMention(detectMention(el.value, el.selectionStart ?? el.value.length));
    },
    [enabled],
  );

  const accept = useCallback(
    (doc: WikiDoc) => {
      const el = textareaRef.current;
      if (!el || !mention) return;
      const caret = el.selectionStart ?? value.length;
      const ref = `[[${doc.slug}]] `;
      onChange(value.slice(0, mention.start) + ref + value.slice(caret));
      setMention(null);
      const next = mention.start + ref.length;
      requestAnimationFrame(() => {
        el.focus();
        el.setSelectionRange(next, next);
      });
    },
    [mention, onChange, textareaRef, value],
  );

  // Close on outside click, leaving clicks on the textarea itself alone.
  useEffect(() => {
    if (!open) return;
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node) && e.target !== textareaRef.current) {
        setMention(null);
      }
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [open, textareaRef]);

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent): boolean => {
      if (!open) return false;
      if (e.key === "ArrowDown") {
        e.preventDefault();
        setActiveIdx((i) => (i + 1) % matches.length);
        return true;
      }
      if (e.key === "ArrowUp") {
        e.preventDefault();
        setActiveIdx((i) => (i - 1 + matches.length) % matches.length);
        return true;
      }
      if (e.key === "Enter" || e.key === "Tab") {
        e.preventDefault();
        accept(matches[activeIdx]);
        return true;
      }
      if (e.key === "Escape") {
        e.preventDefault();
        setMention(null);
        return true;
      }
      return false;
    },
    [accept, activeIdx, matches, open],
  );

  const menu = open ? (
    <div className={styles.menu} ref={wrapRef} role="listbox">
      {matches.map((doc, i) => (
        <button
          key={doc.slug}
          type="button"
          role="option"
          aria-selected={i === activeIdx}
          className={`${styles.item} ${i === activeIdx ? styles.active : ""}`}
          // mousedown fires before the textarea's blur, which would close us.
          onMouseDown={(e) => {
            e.preventDefault();
            accept(doc);
          }}
          onMouseEnter={() => setActiveIdx(i)}
        >
          <span className={styles.slug}>{doc.slug}</span>
          <span className={styles.title}>{doc.title}</span>
        </button>
      ))}
    </div>
  ) : null;

  return { menu, open, sync, close, handleKeyDown };
}
