import { useState } from "react";
import { useTranslation } from "react-i18next";
import { CopyButton } from "../CopyButton";
import styles from "./ExpandableText.module.css";

/** Characters shown before the reader asks for more. */
const PREVIEW_CHARS = 2000;

/**
 * Ceiling on what we will ever put in the DOM at once.
 *
 * Real text results top out around 55k characters, so this is never reached in
 * practice — it exists so that one pathological result cannot lock the webview.
 * Past it, the copy button is the way to get the whole thing.
 */
const RENDER_CAP = 200_000;

/**
 * A block of tool output: previewed, expandable, always copyable.
 *
 * 11% of tool results exceed the preview length, and before this they were cut
 * with a "… N more chars" note and no way to reach the rest.
 */
export function ExpandableText({
  text,
  className,
}: {
  text: string;
  className?: string;
}) {
  const { t } = useTranslation();
  const [expanded, setExpanded] = useState(false);

  const overPreview = text.length > PREVIEW_CHARS;
  const overCap = text.length > RENDER_CAP;
  const shown = expanded
    ? overCap
      ? text.slice(0, RENDER_CAP)
      : text
    : text.slice(0, PREVIEW_CHARS);

  return (
    <div className={styles.root}>
      <pre className={`${styles.text} ${expanded ? styles.text_expanded : ""} ${className ?? ""}`}>
        {shown}
      </pre>
      {(overPreview || text.length > 0) && (
        <div className={styles.footer}>
          {overPreview && (
            <button
              type="button"
              className={styles.toggle}
              onClick={() => setExpanded((v) => !v)}
            >
              {expanded
                ? t("detail.result_collapse")
                : t("detail.result_expand", {
                    count: text.length - PREVIEW_CHARS,
                  })}
            </button>
          )}
          {expanded && overCap && (
            <span className={styles.capped}>
              {t("detail.result_capped", {
                count: text.length - RENDER_CAP,
              })}
            </span>
          )}
          {/* Copies the whole thing, not just what is rendered. */}
          <CopyButton text={text} className={styles.copy} />
        </div>
      )}
    </div>
  );
}
