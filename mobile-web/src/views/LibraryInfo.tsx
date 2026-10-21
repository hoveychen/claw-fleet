// What the AI wrote about one 资料 item — its summary and tags — on the detail
// views (an artifact's ArtifactDetail, a document's WikiDocView). Nothing
// renders when the item has neither: agent-added artifacts and docs the
// backfill has not reached carry none.

import { useState } from "react";
import { Sparkles } from "lucide-react";
import { t } from "../i18n";
import styles from "./LibraryInfo.module.css";

export function LibraryInfo({ summary, tags }: { summary?: string; tags?: string[] }) {
  const [open, setOpen] = useState(false);
  const text = (summary ?? "").trim();
  const chips = (tags ?? []).filter(Boolean);
  if (!text && chips.length === 0) return null;
  return (
    <div className={styles.info} data-library-info>
      {text && (
        <button
          type="button"
          className={styles.summary}
          data-open={open}
          onClick={() => setOpen((v) => !v)}
          aria-label={t("AI 摘要")}
        >
          <Sparkles size={12} strokeWidth={1.8} className={styles.spark} aria-hidden="true" />
          <span className={styles.text}>{text}</span>
        </button>
      )}
      {chips.length > 0 && (
        <div className={styles.tags}>
          {chips.map((tag) => (
            <span key={tag} className={styles.tag}>
              {tag}
            </span>
          ))}
        </div>
      )}
    </div>
  );
}
