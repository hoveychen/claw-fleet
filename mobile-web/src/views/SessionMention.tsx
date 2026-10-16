// `@`-mention UI for the phone: the composer's suggestion list (pick another
// session or relay chain on the same device) and the chip a sent mention
// renders as inside a user bubble.

import { createContext, useCallback, useContext, useMemo, useState, type ReactNode } from "react";
import { Link2 } from "lucide-react";
import {
  mentionTitleOf,
  pickMentionableSessions,
} from "../../../shared-ts/sessionMentions";
import { t } from "../i18n";
import { applyMention, detectMention, mentionSegments, type OpenMention } from "../sessionMention";
import type { SessionInfo } from "../types";
import { timeAgo } from "./TasksView";
import styles from "./SessionMention.module.css";

/**
 * `@` mention state for one composer textarea.
 *
 * `sessions` must already be scoped to the device the composer sends to — a
 * session id names a transcript on one machine only, so offering another
 * device's session would hand the receiving agent an id it cannot resolve.
 */
export function useSessionMention({
  value,
  onChange,
  sessions,
  exclude,
  textareaRef,
}: {
  value: string;
  onChange: (next: string) => void;
  sessions: readonly SessionInfo[];
  exclude?: string[];
  textareaRef: React.RefObject<HTMLTextAreaElement | null>;
}) {
  const [mention, setMention] = useState<OpenMention | null>(null);

  /** Re-read the caret after any edit / selection change. */
  const sync = useCallback((el: HTMLTextAreaElement) => {
    setMention(detectMention(el.value, el.selectionStart ?? el.value.length));
  }, []);

  const excludeKey = (exclude ?? []).join(",");
  const candidates = useMemo(
    () =>
      mention
        ? pickMentionableSessions([...sessions], mention.query, {
            exclude: excludeKey ? excludeKey.split(",") : [],
            limit: 6,
          })
        : [],
    [mention, sessions, excludeKey],
  );

  const accept = useCallback(
    (session: SessionInfo) => {
      if (!mention) return;
      const el = textareaRef.current;
      const caret = el?.selectionStart ?? mention.start + 1 + mention.query.length;
      const next = applyMention(value, mention, caret, session);
      onChange(next.text);
      setMention(null);
      // Keep the keyboard up and put the caret after the inserted token.
      requestAnimationFrame(() => {
        const node = textareaRef.current;
        if (!node) return;
        node.focus();
        node.setSelectionRange(next.caret, next.caret);
      });
    },
    [mention, value, onChange, textareaRef],
  );

  // Deferred: on some touch webviews the tap on a suggestion still blurs the
  // textarea before its click fires; closing the list synchronously on blur
  // would unmount the row under the finger and drop the pick.
  const dismiss = useCallback(() => {
    window.setTimeout(() => setMention(null), 250);
  }, []);

  return { open: mention !== null && candidates.length > 0, candidates, sync, accept, dismiss };
}

/** Suggestion list shown above the composer pill while an `@query` is open. */
export function MentionSuggestions({
  candidates,
  onPick,
}: {
  candidates: SessionInfo[];
  onPick: (s: SessionInfo) => void;
}) {
  return (
    <div className={styles.list} role="listbox" aria-label={t("引用会话")}>
      {candidates.map((s) => {
        const chainLen = s.handoff?.chainLen ?? 0;
        return (
          <button
            key={s.id}
            type="button"
            role="option"
            aria-selected={false}
            className={styles.row}
            // Stop the tap from blurring the textarea: a blur folds the
            // keyboard and the list would jump before the click lands.
            onMouseDown={(e) => e.preventDefault()}
            onClick={() => onPick(s)}
          >
            <span className={styles.title}>{mentionTitleOf(s)}</span>
            <span className={styles.meta}>
              {chainLen > 1 && (
                <span className={styles.chain}>
                  <Link2 size={11} />
                  {t("接力 {0} 棒", chainLen)}
                </span>
              )}
              <span className={styles.workspace}>{s.workspaceName}</span>
              <span>{timeAgo(s.lastActivityMs)}</span>
            </span>
          </button>
        );
      })}
    </div>
  );
}

// ── Rendering a sent mention ─────────────────────────────────────────────────

export interface SessionMentionNav {
  open: (sessionId: string) => void;
  has: (sessionId: string) => boolean;
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
              aria-label={t("打开会话 {0}", seg.title)}
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
  return <div className={styles.refsNote}>{t("已附上 {0} 个会话的摘要", count)}</div>;
}
