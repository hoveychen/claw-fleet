import { useTranslation } from "react-i18next";

import styles from "./TabPanes.module.css";

/** The library's AI summary and tags for a doc or artifact, shown above its
 *  body in the reader panes. Renders nothing until a summary exists. */
export function LibrarySummary({ summary, tags }: { summary?: string; tags?: string[] }) {
  const { t } = useTranslation();
  const text = summary?.trim() ?? "";
  const chips = tags ?? [];
  if (!text && chips.length === 0) return null;
  return (
    <div className={styles.summary_strip} aria-label={t("library.summary", "摘要")}>
      {text && <p className={styles.summary_text}>{text}</p>}
      {chips.length > 0 && (
        <div className={styles.summary_tags} aria-label={t("library.tags", "标签")}>
          {chips.map((tag) => (
            <span key={tag} className={styles.summary_tag}>{tag}</span>
          ))}
        </div>
      )}
    </div>
  );
}
