import { useTranslation } from "react-i18next";
import { BookOpen, MessagesSquare } from "lucide-react";
import { useSessionsStore, useUIStore } from "../store";
import { revealSlugInWikiPage, useWikiDocsStore } from "../hooks/useWikiDocs";
import type { PromptRef } from "../../../shared-ts/sessionMentions";
import styles from "./RefChip.module.css";

/**
 * A referenced session or wiki doc, as a chip: in the composer's attachment
 * row (with a remove button) and on a sent user turn (without one).
 *
 * Clicking the label opens the target — the session's task tab, or the doc on
 * the wiki page. A session that is no longer in the scan reads as inert.
 */
export function RefChip({
  value: r,
  onRemove,
  disabled,
}: {
  value: PromptRef;
  onRemove?: () => void;
  disabled?: boolean;
}) {
  const { t } = useTranslation();
  const known = useSessionsStore((s) =>
    r.kind === "session" ? s.sessions.some((x) => x.id === r.id) : true,
  );
  // A sent message only carries the slug; use the doc's title when the shared
  // list already has it (never fetched just for this).
  const docTitle = useWikiDocsStore((s) =>
    r.kind === "wiki" && !r.title ? s.docs.find((d) => d.slug === r.slug)?.title : undefined,
  );
  const label = r.kind === "session" ? r.title : r.title || docTitle || r.slug;
  const Icon = r.kind === "session" ? MessagesSquare : BookOpen;
  const open = () => {
    if (r.kind === "session") useUIStore.getState().requestOpenTask(r.id);
    else revealSlugInWikiPage(r.slug);
  };
  const hint =
    r.kind === "session"
      ? known
        ? t("composer.ref_open_session", "打开这个会话")
        : t("composer.ref_session_gone", "会话已不在列表中")
      : `${t("composer.ref_open_wiki", "在知识库打开")} · ${r.slug}`;
  return (
    <span className={`${styles.chip} ${known ? "" : styles.gone}`} data-kind={r.kind}>
      <button
        type="button"
        className={styles.open}
        onClick={open}
        disabled={!known}
        title={hint}
        aria-label={`${hint}: ${label}`}
      >
        <Icon size={12} className={styles.icon} />
        <span className={styles.name}>{label}</span>
      </button>
      {onRemove && (
        <button
          type="button"
          className={styles.remove}
          onClick={onRemove}
          disabled={disabled}
          title={t("composer.ref_remove", "移除引用")}
          aria-label={t("composer.ref_remove", "移除引用")}
        >
          ×
        </button>
      )}
    </span>
  );
}
