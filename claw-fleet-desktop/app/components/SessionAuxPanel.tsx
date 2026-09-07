import type { ReactNode } from "react";
import { useTranslation } from "react-i18next";
import styles from "./SessionDetail.module.css";

/**
 * The session detail's auxiliary column — the shell only.
 *
 * What goes *in* it (a facet panel, a doc, the live-subagent cards) is decided
 * by `SessionDetail`, which owns that data; this component owns the column
 * itself: its width and drag handle, its header, and the one behaviour that
 * isn't layout — collapsing to an overlay drawer when the pane it lives in is
 * too narrow to hold two readable columns.
 *
 * That narrow case is not hypothetical: the same `SessionDetail` renders inside
 * `DecisionPanel`'s inline detail column and inside a 4-way split of the 任务
 * page, where a half can be ~300px. Splitting that in two would leave neither
 * side readable, so below the threshold the panel floats over the conversation
 * instead of taking a share of it.
 */
export function SessionAuxPanel({
  overlay,
  width,
  isDragging,
  onResizeStart,
  title,
  onClose,
  children,
}: {
  /** Float over the conversation instead of sitting beside it. */
  overlay: boolean;
  /** Column width in px. Ignored in overlay mode, which sizes itself. */
  width: number;
  isDragging: boolean;
  onResizeStart: (e: React.MouseEvent) => void;
  title: ReactNode;
  onClose: () => void;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  return (
    <>
      {/* Only in overlay mode: the conversation underneath is still visible, so
          a click outside is the natural way back to it. */}
      {overlay && <div className={styles.aux_scrim} onClick={onClose} />}
      <aside
        className={`${styles.aux} ${overlay ? styles.aux_overlay : ""} ${isDragging ? styles.aux_dragging : ""}`}
        style={overlay ? undefined : { width }}
      >
        {!overlay && (
          <div
            className={styles.aux_resizer}
            onMouseDown={onResizeStart}
            role="separator"
            aria-orientation="vertical"
          />
        )}
        <div className={styles.aux_head}>
          <div className={styles.aux_title}>{title}</div>
          <button
            type="button"
            className={styles.aux_close}
            onClick={onClose}
            title={t("common.close", "关闭")}
            aria-label={t("common.close", "关闭")}
          >
            ✕
          </button>
        </div>
        <div className={styles.aux_body}>{children}</div>
      </aside>
    </>
  );
}
