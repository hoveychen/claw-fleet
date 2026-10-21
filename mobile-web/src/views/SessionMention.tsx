// `@`-mention UI for the phone: the composer's suggestion list (pick another
// session or relay chain on the same device) and the chip a sent mention
// renders as inside a user bubble.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { BookOpen, Link2, MessagesSquare, Paperclip } from "lucide-react";
import {
  mentionTitleOf,
  pickMentionableSessions,
  promptRefKey,
  type PromptRef,
} from "../../../shared-ts/sessionMentions";
import { t } from "../i18n";
import { applyMention, detectMention, mentionSegments, type OpenMention } from "../sessionMention";
import type { FleetTransport } from "../transport";
import type { SessionInfo, WikiDoc } from "../types";
import { listWikiDocs } from "../wiki";
import { fetchWikiDoc } from "../libraryApi";
import { timeAgo } from "./TasksView";
import styles from "./SessionMention.module.css";

/** One row of a mention / reference picker. */
export type MentionCandidate =
  | { kind: "session"; session: SessionInfo }
  | { kind: "wiki"; doc: WikiDoc };

/** The chip a picked row becomes. */
export function candidateRef(c: MentionCandidate): PromptRef {
  return c.kind === "wiki"
    ? { kind: "wiki", slug: c.doc.slug, title: c.doc.title }
    : {
        kind: "session",
        id: c.session.id,
        title: mentionTitleOf(c.session) ?? c.session.id.slice(0, 8),
      };
}

/** Rank docs whose slug or title contains the query; slug matches first
 *  (mirrors the desktop's `filterWikiDocs`). */
export function filterWikiDocs(docs: readonly WikiDoc[], query: string, limit: number): WikiDoc[] {
  const q = query.trim().toLowerCase();
  if (!q) return docs.slice(0, limit);
  const bySlug = docs.filter((d) => d.slug.toLowerCase().includes(q));
  const byTitle = docs.filter(
    (d) => !d.slug.toLowerCase().includes(q) && d.title.toLowerCase().includes(q),
  );
  return [...bySlug, ...byTitle].slice(0, limit);
}

/**
 * The target device's wiki doc list, fetched once the first time `enabled`
 * turns true (an `@` opened, or the reference picker did). `null` while the
 * first fetch is in flight; `[]` when there is no client or it failed.
 */
export function useWikiDocList(client: FleetTransport | null, enabled: boolean): WikiDoc[] | null {
  const [docs, setDocs] = useState<WikiDoc[] | null>(null);
  const [asked, setAsked] = useState(false);
  useEffect(() => {
    if (enabled) setAsked(true);
  }, [enabled]);
  useEffect(() => {
    if (!asked) return;
    if (!client) {
      setDocs([]);
      return;
    }
    let cancelled = false;
    listWikiDocs(client)
      .then((d) => !cancelled && setDocs(d ?? []))
      .catch(() => !cancelled && setDocs([]));
    return () => {
      cancelled = true;
    };
  }, [asked, client]);
  return docs;
}

/**
 * `@` mention state for one composer textarea.
 *
 * `sessions` must already be scoped to the device the composer sends to — a
 * session id names a transcript on one machine only, so offering another
 * device's session would hand the receiving agent an id it cannot resolve.
 *
 * With `onPickRef` (the composers that hold references as chips) a pick drops
 * the typed `@query` and hands the ref over; wiki docs of `client`'s device
 * are offered below the sessions. Without it, a session pick inserts its link.
 */
export function useSessionMention({
  value,
  onChange,
  sessions,
  exclude,
  textareaRef,
  onPickRef,
  client,
}: {
  value: string;
  onChange: (next: string) => void;
  sessions: readonly SessionInfo[];
  exclude?: string[];
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
  onPickRef?: (ref: PromptRef) => void;
  client?: FleetTransport | null;
}) {
  const [mention, setMention] = useState<OpenMention | null>(null);
  const wikiDocs = useWikiDocList(client ?? null, Boolean(onPickRef) && mention !== null);

  /** Re-read the caret after any edit / selection change. */
  const sync = useCallback((el: HTMLTextAreaElement) => {
    setMention(detectMention(el.value, el.selectionStart ?? el.value.length));
  }, []);

  const excludeKey = (exclude ?? []).join(",");
  const candidates = useMemo((): MentionCandidate[] => {
    if (!mention) return [];
    const rows: MentionCandidate[] = pickMentionableSessions([...sessions], mention.query, {
      exclude: excludeKey ? excludeKey.split(",") : [],
      limit: onPickRef ? 4 : 6,
    }).map((s) => ({ kind: "session", session: s }));
    if (onPickRef && wikiDocs) {
      for (const d of filterWikiDocs(wikiDocs, mention.query, 4)) rows.push({ kind: "wiki", doc: d });
    }
    return rows;
  }, [mention, sessions, excludeKey, onPickRef, wikiDocs]);

  const accept = useCallback(
    (c: MentionCandidate) => {
      if (!mention) return;
      const el = textareaRef.current;
      const caret = el?.selectionStart ?? mention.start + 1 + mention.query.length;
      let next: { text: string; caret: number };
      if (onPickRef) {
        next = { text: value.slice(0, mention.start) + value.slice(caret), caret: mention.start };
        onPickRef(candidateRef(c));
      } else if (c.kind === "session") {
        next = applyMention(value, mention, caret, c.session);
      } else {
        return;
      }
      onChange(next.text);
      setMention(null);
      // Keep the keyboard up and put the caret where the pick left it.
      requestAnimationFrame(() => {
        const node = textareaRef.current;
        if (!node) return;
        node.focus();
        node.setSelectionRange(next.caret, next.caret);
      });
    },
    [mention, value, onChange, onPickRef, textareaRef],
  );

  // Deferred: on some touch webviews the tap on a suggestion still blurs the
  // textarea before its click fires; closing the list synchronously on blur
  // would unmount the row under the finger and drop the pick.
  const dismiss = useCallback(() => {
    window.setTimeout(() => setMention(null), 250);
  }, []);

  return { open: mention !== null && candidates.length > 0, candidates, sync, accept, dismiss };
}

/** One picker row's content. */
function CandidateLabel({ c }: { c: MentionCandidate }) {
  if (c.kind === "wiki") {
    return (
      <>
        <span className={styles.title}>
          <BookOpen size={13} className={styles.rowIcon} />
          {c.doc.title || c.doc.slug}
        </span>
        <span className={styles.meta}>
          <span className={styles.workspace}>{c.doc.slug}</span>
        </span>
      </>
    );
  }
  const s = c.session;
  const chainLen = s.handoff?.chainLen ?? 0;
  return (
    <>
      <span className={styles.title}>{mentionTitleOf(s)}</span>
      <span className={styles.meta}>
        {chainLen > 1 && (
          <span className={styles.chain}>
            <Link2 size={11} />
            {t("续接 {0} 段", chainLen)}
          </span>
        )}
        <span className={styles.workspace}>{s.workspaceName}</span>
        <span>{timeAgo(s.lastActivityMs)}</span>
      </span>
    </>
  );
}

function candidateKey(c: MentionCandidate): string {
  return c.kind === "wiki" ? `w:${c.doc.slug}` : `s:${c.session.id}`;
}

/** Suggestion list shown above the composer pill while an `@query` is open. */
export function MentionSuggestions({
  candidates,
  onPick,
}: {
  candidates: MentionCandidate[];
  onPick: (c: MentionCandidate) => void;
}) {
  return (
    <div className={styles.list} role="listbox" aria-label={t("引用任务")}>
      {candidates.map((c) => (
        <button
          key={candidateKey(c)}
          type="button"
          role="option"
          aria-selected={false}
          className={styles.row}
          // Stop the tap from blurring the textarea: a blur folds the
          // keyboard and the list would jump before the click lands.
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onPick(c)}
        >
          <CandidateLabel c={c} />
        </button>
      ))}
    </div>
  );
}

/**
 * The "+" menu's 引用会话 / 引用知识库 picker: a bottom sheet with a search box
 * over the same rows the `@` list offers.
 */
export function RefPickerSheet({
  kind,
  sessions,
  exclude,
  client,
  onPick,
  onClose,
}: {
  kind: "session" | "wiki";
  sessions: readonly SessionInfo[];
  exclude?: string[];
  client: FleetTransport | null;
  onPick: (ref: PromptRef) => void;
  onClose: () => void;
}) {
  const [query, setQuery] = useState("");
  const docs = useWikiDocList(client, kind === "wiki");
  const rows: MentionCandidate[] =
    kind === "session"
      ? pickMentionableSessions([...sessions], query, { exclude, limit: 40 }).map((s) => ({
          kind: "session",
          session: s,
        }))
      : filterWikiDocs(docs ?? [], query, 40).map((d) => ({ kind: "wiki", doc: d }));
  const loading = kind === "wiki" && docs === null;
  return (
    <>
      <div className={styles.sheetBackdrop} onClick={onClose} />
      <div
        className={styles.sheet}
        role="dialog"
        aria-modal="true"
        aria-label={kind === "session" ? t("引用任务") : t("引用知识库")}
      >
        <div className={styles.sheetTitle}>{kind === "session" ? t("引用任务") : t("引用知识库")}</div>
        <input
          className={styles.search}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          placeholder={kind === "session" ? t("搜索任务标题、项目…") : t("搜索知识库文档…")}
        />
        <div className={styles.sheetList}>
          {loading ? (
            <div className={styles.empty}>{t("加载中…")}</div>
          ) : rows.length === 0 ? (
            <div className={styles.empty}>{t("没有匹配的结果")}</div>
          ) : (
            rows.map((c) => (
              <button
                key={candidateKey(c)}
                type="button"
                className={styles.row}
                onClick={() => {
                  onPick(candidateRef(c));
                  onClose();
                }}
              >
                <CandidateLabel c={c} />
              </button>
            ))
          )}
        </div>
      </div>
    </>
  );
}

/** The "+" button's small menu: files, or a reference. */
export function PlusMenu({
  onFiles,
  onRef,
  onClose,
  allowSessions = true,
}: {
  onFiles: () => void;
  onRef: (kind: "session" | "wiki") => void;
  onClose: () => void;
  allowSessions?: boolean;
}) {
  return (
    <>
      <div className={styles.menuBackdrop} onClick={onClose} />
      <div className={styles.plusMenu} role="menu">
        <button type="button" role="menuitem" className={styles.menuItem} onClick={onFiles}>
          <Paperclip size={15} />
          {t("附件")}
        </button>
        {allowSessions && (
          <button
            type="button"
            role="menuitem"
            className={styles.menuItem}
            onClick={() => onRef("session")}
          >
            <MessagesSquare size={15} />
            {t("引用任务")}
          </button>
        )}
        <button type="button" role="menuitem" className={styles.menuItem} onClick={() => onRef("wiki")}>
          <BookOpen size={15} />
          {t("引用知识库")}
        </button>
      </div>
    </>
  );
}

/** Referenced sessions / wiki docs as chips — in a composer (removable) or on
 *  a sent user turn. A tap opens the target when this view knows how. */
export function RefChips({
  refs,
  onRemove,
}: {
  refs: readonly PromptRef[];
  onRemove?: (r: PromptRef) => void;
}) {
  const nav = useContext(SessionMentionNavContext);
  // A sent `[[slug]]` carries no title; ask the view to fetch those docs.
  const untitledWiki = refs
    .flatMap((r) => (r.kind === "wiki" && !r.title ? [r.slug] : []))
    .join("\n");
  const wantWikiTitles = nav?.wantWikiTitles;
  useEffect(() => {
    if (untitledWiki) wantWikiTitles?.(untitledWiki.split("\n"));
  }, [untitledWiki, wantWikiTitles]);
  if (refs.length === 0) return null;
  return (
    <div className={styles.refChips}>
      {refs.map((r) => {
        const label =
          r.kind === "session" ? r.title : r.title || nav?.wikiTitle?.(r.slug) || r.slug;
        const Icon = r.kind === "session" ? MessagesSquare : BookOpen;
        const openable =
          r.kind === "session" ? Boolean(nav?.has(r.id)) : Boolean(nav?.openWiki);
        return (
          <span key={promptRefKey(r)} className={styles.refChip} data-inert={!openable || undefined}>
            <button
              type="button"
              className={styles.refOpen}
              disabled={!openable}
              onClick={() =>
                r.kind === "session" ? nav?.open(r.id) : nav?.openWiki?.(r.slug)
              }
              aria-label={r.kind === "session" ? t("打开任务 {0}", label) : t("打开文档 {0}", label)}
            >
              <Icon size={13} />
              <span className={styles.refName}>{label}</span>
            </button>
            {onRemove && (
              <button
                type="button"
                className={styles.refRemove}
                onClick={() => onRemove(r)}
                aria-label={t("移除引用")}
              >
                ×
              </button>
            )}
          </span>
        );
      })}
    </div>
  );
}

// ── Rendering a sent mention ─────────────────────────────────────────────────

export interface SessionMentionNav {
  open: (sessionId: string) => void;
  has: (sessionId: string) => boolean;
  /** Open a wiki doc of this view's device by slug. */
  openWiki?: (slug: string) => void;
  /** Title of a wiki doc by slug, once that doc's record has arrived. */
  wikiTitle?: (slug: string) => string | undefined;
  /** Chips need `wikiTitle` for these slugs; the view fetches each doc once. */
  wantWikiTitles?: (slugs: string[]) => void;
}

/** `wikiTitle` / `wantWikiTitles` for a view's nav: fetches just the docs a
 *  chip asks a title for (`wiki_doc`), not the device's whole wiki list. */
export function useWikiTitles(client: FleetTransport | null) {
  const [titles, setTitles] = useState<Record<string, string>>({});
  const asked = useRef<{ client: FleetTransport | null; slugs: Set<string> }>({ client, slugs: new Set() });
  const wantWikiTitles = useCallback(
    (slugs: string[]) => {
      if (!client) return;
      if (asked.current.client !== client) asked.current = { client, slugs: new Set() };
      for (const slug of slugs) {
        if (asked.current.slugs.has(slug)) continue;
        asked.current.slugs.add(slug);
        fetchWikiDoc(client, slug)
          .then((d) => d.title && setTitles((t) => ({ ...t, [slug]: d.title })))
          .catch(() => {});
      }
    },
    [client],
  );
  // Another device's titles are not this one's.
  useEffect(() => setTitles({}), [client]);
  const wikiTitle = useCallback((slug: string) => titles[slug] || undefined, [titles]);
  return { wikiTitle, wantWikiTitles };
}

const SessionMentionNavContext = createContext<SessionMentionNav | null>(null);

export function SessionMentionNavProvider({
  nav,
  children,
}: {
  nav: SessionMentionNav;
  children: ReactNode;
}) {
  return (
    <SessionMentionNavContext.Provider value={nav}>{children}</SessionMentionNavContext.Provider>
  );
}

/** User-message text with each `[@title](fleet-session:id)` link drawn as a
 *  chip. A chip whose session is in this device's snapshot opens it on tap;
 *  one that is not (aged out, other device) stays a plain, inert chip. */
export function MentionText({ text }: { text: string }) {
  const nav = useContext(SessionMentionNavContext);
  const segments = useMemo(() => mentionSegments(text), [text]);
  if (!segments.some((s) => s.kind === "mention")) return <>{text}</>;
  return (
    <>
      {segments.map((seg, i) => {
        if (seg.kind === "text") return <span key={i}>{seg.text}</span>;
        const label = `@${seg.title}`;
        if (nav?.has(seg.id)) {
          return (
            <button
              key={i}
              type="button"
              className={styles.chip}
              onClick={() => nav.open(seg.id)}
              aria-label={t("打开任务 {0}", seg.title)}
            >
              {label}
            </button>
          );
        }
        return (
          <span key={i} className={styles.chip} data-inert="true">
            {label}
          </span>
        );
      })}
    </>
  );
}

/** The muted line that stands in for the `<fleet-referenced-sessions>` block. */
export function RefsNote({ count }: { count: number }) {
  if (count <= 0) return null;
  return <div className={styles.refsNote}>{t("已附上 {0} 个任务的摘要", count)}</div>;
}
