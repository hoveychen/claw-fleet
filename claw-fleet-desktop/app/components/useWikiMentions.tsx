// `@`-mention autocomplete over the wiki — and, for composers that start or
// continue a session, over other sessions — shared by every prompt box.
//
// Picking a doc inserts a `[[slug]]` reference rather than a path: the slug is
// the doc's stable address (it survives version bumps and the wiki's on-disk
// escaping), and `fleet wiki cat <slug>` is what the agent runs to read it.
//
// Picking a session inserts `[@title](fleet-session:<id>)`; core's
// `session_refs` inlines that chain's summary and grants history access when
// the prompt is sent (see shared-ts/sessionMentions.ts).
//
// A composer that holds references as chips passes `onPickRef`: the pick then
// removes the typed `@query` and hands the ref over instead of inserting text.

import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useRef, useState, type RefObject } from "react";
import { useTranslation } from "react-i18next";
import type { WikiDoc } from "./WikiView";
import { useAutoFlip } from "./useAutoFlip";
import { Spinner } from "./loading";
import { useDelayedFlag } from "../hooks/useDelayedFlag";
import { useSessionsStore } from "../store";
import type { SessionInfo } from "../types";
import {
  mentionTitleOf,
  pickMentionableSessions,
  sessionMentionToken,
  type PromptRef,
} from "../../../shared-ts/sessionMentions";
import styles from "./useWikiMentions.module.css";

/** Longest `@…` run still treated as a mention query rather than prose. */
const MAX_MENTION_QUERY = 48;
const MAX_MENTION_RESULTS = 8;
const MAX_SESSION_RESULTS = 5;

/** Opt-in for session mentions: `exclude` lists the session being continued. */
export interface SessionMentionOptions {
  exclude?: string[];
}

export type MentionItem =
  | { kind: "session"; key: string; session: SessionInfo }
  | { kind: "wiki"; key: string; doc: WikiDoc };

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
export function filterWikiDocs(
  docs: WikiDoc[],
  query: string,
  limit = MAX_MENTION_RESULTS,
): WikiDoc[] {
  const q = query.trim().toLowerCase();
  if (!q) return docs.slice(0, limit);
  const bySlug: WikiDoc[] = [];
  const byTitle: WikiDoc[] = [];
  for (const d of docs) {
    if (d.slug.toLowerCase().includes(q)) bySlug.push(d);
    else if (d.title.toLowerCase().includes(q)) byTitle.push(d);
  }
  return [...bySlug, ...byTitle].slice(0, limit);
}

/** The chip a picked item becomes. */
export function mentionItemRef(item: MentionItem): PromptRef {
  return item.kind === "wiki"
    ? { kind: "wiki", slug: item.doc.slug, title: item.doc.title }
    : {
        kind: "session",
        id: item.session.id,
        title: mentionTitleOf(item.session) ?? item.session.id.slice(0, 8),
      };
}

/** The label of one picker row (shared with the "+" menu's reference picker). */
export function MentionItemLabel({ item }: { item: MentionItem }) {
  const { t } = useTranslation();
  if (item.kind === "wiki") {
    return (
      <>
        <span className={styles.slug}>{item.doc.slug}</span>
        <span className={styles.title}>{item.doc.title}</span>
      </>
    );
  }
  return (
    <>
      <span className={styles.session_title}>{mentionTitleOf(item.session)}</span>
      <span className={styles.title}>
        {item.session.workspaceName}
        {item.session.handoff
          ? ` · ${t("wiki.mention_chain", "接力 {{n}} 棒", { n: item.session.handoff.chainLen })}`
          : ""}
      </span>
    </>
  );
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
 * @param sessionOpts When set, sessions are offered above the wiki docs.
 * @param onPickRef When set, a pick becomes a chip instead of inserted text.
 */
export function useWikiMentions(
  enabled: boolean,
  value: string,
  onChange: (next: string) => void,
  textareaRef: RefObject<HTMLTextAreaElement | null>,
  sessionOpts?: SessionMentionOptions | null,
  onPickRef?: (ref: PromptRef) => void,
): WikiMentions {
  const { t } = useTranslation();
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

  // Sessions are read from the store at match time rather than subscribed to:
  // the roster changes on every scan, and the composer has no reason to
  // re-render for that while no `@` is open.
  const sessionMatches =
    enabled && mention && sessionOpts
      ? pickMentionableSessions(useSessionsStore.getState().sessions, mention.query, {
          exclude: sessionOpts.exclude,
          limit: MAX_SESSION_RESULTS,
        })
      : [];
  const docMatches = mention && docs ? filterWikiDocs(docs, mention.query) : [];
  const matches: MentionItem[] = [
    ...sessionMatches.map((s): MentionItem => ({ kind: "session", key: `s:${s.id}`, session: s })),
    ...docMatches.map((d): MentionItem => ({ kind: "wiki", key: `w:${d.slug}`, doc: d })),
  ];
  const open = Boolean(mention) && matches.length > 0;
  // The first `@` waits on the list: show a one-row spinner instead of nothing.
  // It does not claim the keyboard (`open` stays false), so Enter still sends.
  const showLoading = useDelayedFlag(
    enabled && Boolean(mention) && docs === null && sessionMatches.length === 0,
  );
  const visible = open || showLoading;

  // Prefer opening upward (the composer usually sits low), but fall back to
  // below when the composer is near the top of the window. The anchor defaults
  // to the menu's offsetParent — the composer it spans.
  // `matches.length` re-measures as the result list grows or shrinks.
  const side = useAutoFlip(visible, "above", wrapRef, undefined, matches.length);

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
    (item: MentionItem) => {
      const el = textareaRef.current;
      if (!el || !mention) return;
      const caret = el.selectionStart ?? value.length;
      if (onPickRef) {
        onChange(value.slice(0, mention.start) + value.slice(caret));
        onPickRef(mentionItemRef(item));
        setMention(null);
        const at = mention.start;
        requestAnimationFrame(() => {
          el.focus();
          el.setSelectionRange(at, at);
        });
        return;
      }
      const ref =
        item.kind === "wiki"
          ? `[[${item.doc.slug}]] `
          : `${sessionMentionToken(mentionTitleOf(item.session) ?? "", item.session.id)} `;
      onChange(value.slice(0, mention.start) + ref + value.slice(caret));
      setMention(null);
      const next = mention.start + ref.length;
      requestAnimationFrame(() => {
        el.focus();
        el.setSelectionRange(next, next);
      });
    },
    [mention, onChange, onPickRef, textareaRef, value],
  );

  // Close on outside click, leaving clicks on the textarea itself alone.
  useEffect(() => {
    if (!visible) return;
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node) && e.target !== textareaRef.current) {
        setMention(null);
      }
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [visible, textareaRef]);

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

  const menu = showLoading && !open ? (
    <div
      className={`${styles.menu} ${side === "above" ? styles.above : styles.below}`}
      ref={wrapRef}
    >
      <div className={styles.loading}>
        <Spinner size={12} />
        {t("wiki.mention_loading", "正在加载知识库…")}
      </div>
    </div>
  ) : open ? (
    <div
      className={`${styles.menu} ${side === "above" ? styles.above : styles.below}`}
      ref={wrapRef}
      role="listbox"
    >
      {matches.map((item, i) => (
        <div key={item.key} className={styles.row}>
          {i === 0 || matches[i - 1].kind !== item.kind ? (
            <div className={styles.section}>
              {item.kind === "session"
                ? t("wiki.mention_section_sessions", "会话")
                : t("wiki.mention_section_wiki", "知识库")}
            </div>
          ) : null}
          <button
            type="button"
            role="option"
            aria-selected={i === activeIdx}
            className={`${styles.item} ${i === activeIdx ? styles.active : ""}`}
            // mousedown fires before the textarea's blur, which would close us.
            onMouseDown={(e) => {
              e.preventDefault();
              accept(item);
            }}
            onMouseEnter={() => setActiveIdx(i)}
          >
            <MentionItemLabel item={item} />
          </button>
        </div>
      ))}
    </div>
  ) : null;

  return { menu, open, sync, close, handleKeyDown };
}
