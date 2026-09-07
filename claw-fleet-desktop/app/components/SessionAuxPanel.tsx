import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import styles from "./SessionDetail.module.css";

/**
 * The session detail's drawer — the *lookup* half of the auxiliary surfaces.
 *
 * It floats over the conversation from the right edge, so opening a lookup
 * surface never narrows or reflows the transcript underneath it, and it stops
 * short of the card rail (see `--rail-space`): the rail is the other layer of
 * information and stays readable while you read this one.
 *
 * One thing at a time, named in the head — a facet the reader picked from the
 * header menu, or the full-width reader for one card in the rail. It has no tab
 * strip on purpose: the strip it used to have put a running subagent, a token
 * receipt and an open file in one row of equals, which is exactly the mixing of
 * two levels this split undoes. What goes *in* it is decided by
 * `SessionDetail`, which owns that data.
 *
 * The same overlay shape is used at every pane width, which keeps the
 * interaction identical in the standalone detail, `DecisionPanel`, and a 4-way
 * split of the 任务 page.
 */
export function SessionAuxPanel({
  title,
  onClose,
  children,
}: {
  /** What is showing — the facet's label or the doc's name. */
  title: string;
  onClose: () => void;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <>
      <div className={styles.aux_scrim} onClick={onClose} />
      <aside className={`${styles.aux} ${styles.aux_overlay}`}>
        {/* Owns the top of the drawer, and on a frameless window the strip
            above it, so it carries its own drag region for the same reason the
            hero banner does (Tauri's shim reads e.target, not an ancestor). */}
        <div className={styles.aux_head} data-tauri-drag-region>
          <span className={styles.aux_title} title={title}>
            {title}
          </span>
          <button
            type="button"
            className={styles.aux_close}
            onClick={onClose}
            title={t("detail.drawer_hide", "收起详情抽屉")}
            aria-label={t("detail.drawer_hide", "收起详情抽屉")}
          >
            ✕
          </button>
        </div>
        <div className={styles.aux_body}>{children}</div>
      </aside>
    </>
  );
}
