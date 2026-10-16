import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionsStore } from "../store";
import { useWikiDocs } from "../hooks/useWikiDocs";
import { pickMentionableSessions, type PromptRef } from "../../../shared-ts/sessionMentions";
import { useAutoFlip } from "./useAutoFlip";
import { Spinner } from "./loading";
import {
  filterWikiDocs,
  MentionItemLabel,
  mentionItemRef,
  type MentionItem,
} from "./useWikiMentions";
import mentionStyles from "./useWikiMentions.module.css";
import styles from "./RefPicker.module.css";

const MAX_RESULTS = 30;

/**
 * The composer "+" menu's "引用会话" / "引用知识库" picker: a search box over
 * the same rows the `@` picker offers. Floats off the composer, which must be
 * `position: relative`, like the `@` menu.
 */
export function RefPicker({
  kind,
  exclude,
  onPick,
  onClose,
}: {
  kind: "session" | "wiki";
  /** Sessions not to offer (the one being continued). */
  exclude?: string[];
  onPick: (ref: PromptRef) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [activeIdx, setActiveIdx] = useState(0);
  const wrapRef = useRef<HTMLDivElement | null>(null);
  const sessions = useSessionsStore((s) => s.sessions);
  const { docs, loaded } = useWikiDocs();

  const items: MentionItem[] =
    kind === "session"
      ? pickMentionableSessions(sessions, query, { exclude, limit: MAX_RESULTS }).map((s) => ({
          kind: "session",
          key: `s:${s.id}`,
          session: s,
        }))
      : filterWikiDocs(docs, query, MAX_RESULTS).map((d) => ({
          kind: "wiki",
          key: `w:${d.slug}`,
          doc: d,
        }));
  const side = useAutoFlip(true, "above", wrapRef, undefined, items.length);

  useEffect(() => setActiveIdx(0), [query]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!wrapRef.current?.contains(e.target as Node)) onClose();
    };
    window.addEventListener("mousedown", onDown);
    return () => window.removeEventListener("mousedown", onDown);
  }, [onClose]);

  const pick = (item: MentionItem | undefined) => {
    if (!item) return;
    onPick(mentionItemRef(item));
    onClose();
  };

  const onKeyDown = (e: React.KeyboardEvent) => {
    if (e.nativeEvent.isComposing || e.keyCode === 229) return;
    if (e.key === "ArrowDown" && items.length > 0) {
      e.preventDefault();
      setActiveIdx((i) => (i + 1) % items.length);
    } else if (e.key === "ArrowUp" && items.length > 0) {
      e.preventDefault();
      setActiveIdx((i) => (i - 1 + items.length) % items.length);
    } else if (e.key === "Enter") {
      e.preventDefault();
      pick(items[activeIdx]);
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onClose();
    }
  };

  const loading = kind === "wiki" && !loaded;
  return (
    <div
      ref={wrapRef}
      className={`${mentionStyles.menu} ${side === "above" ? mentionStyles.above : mentionStyles.below} ${styles.picker}`}
    >
      <input
        className={styles.search}
        autoFocus
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onKeyDown={onKeyDown}
        placeholder={
          kind === "session"
            ? t("composer.ref_search_sessions", "搜索会话标题、项目…")
            : t("composer.ref_search_wiki", "搜索知识库文档…")
        }
        aria-label={
          kind === "session"
            ? t("composer.ref_session", "引用会话")
            : t("composer.ref_wiki", "引用知识库")
        }
      />
      <div className={styles.list} role="listbox">
        {loading ? (
          <div className={mentionStyles.loading}>
            <Spinner size={12} />
            {t("wiki.mention_loading", "正在加载知识库…")}
          </div>
        ) : items.length === 0 ? (
          <div className={styles.empty}>{t("composer.ref_empty", "没有匹配的结果")}</div>
        ) : (
          items.map((item, i) => (
            <button
              key={item.key}
              type="button"
              role="option"
              aria-selected={i === activeIdx}
              className={`${mentionStyles.item} ${styles.item} ${i === activeIdx ? mentionStyles.active : ""}`}
              onMouseDown={(e) => {
                e.preventDefault();
                pick(item);
              }}
              onMouseEnter={() => setActiveIdx(i)}
            >
              <MentionItemLabel item={item} />
            </button>
          ))
        )}
      </div>
    </div>
  );
}
