import { useEffect, useRef, type ReactNode } from "react";
import { useTranslation } from "react-i18next";
import styles from "./SessionDetail.module.css";

/** One tab in the auxiliary column's strip. */
export interface AuxTab {
  id: string;
  label: string;
  /** Docs (the things the agent named) can be closed; facets and the agent
   *  deck come and go with the session's own state, so they cannot. */
  closable?: boolean;
}

/**
 * The session detail's auxiliary column.
 *
 * It owns the column — its width and drag handle, its tab strip, and the one
 * behaviour that isn't layout: collapsing to an overlay drawer when the pane it
 * lives in is too narrow to hold two readable columns. What goes *in* it is
 * decided by `SessionDetail`, which owns that data.
 *
 * Everything the panel can show is one flat strip of tabs — the running agents,
 * the session's facets, and each doc opened from the transcript — rather than
 * sections stacked down the column. Stacking made every one of them shorter
 * than it needed to be; a tab is the honest shape when only one of them is
 * being read at a time.
 *
 * The narrow case is not hypothetical: the same `SessionDetail` renders inside
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
  tabs,
  activeId,
  onPick,
  onCloseTab,
  onClose,
  children,
}: {
  /** Float over the conversation instead of sitting beside it. */
  overlay: boolean;
  /** Column width in px. Ignored in overlay mode, which sizes itself. */
  width: number;
  isDragging: boolean;
  onResizeStart: (e: React.MouseEvent) => void;
  tabs: AuxTab[];
  activeId: string | null;
  onPick: (id: string) => void;
  onCloseTab: (id: string) => void;
  onClose: () => void;
  children: ReactNode;
}) {
  const { t } = useTranslation();
  // Keep the selected tab on screen. Opening a doc from the transcript appends
  // its tab at the far right of a strip that may already be scrolled — without
  // this, the click looks like it did nothing.
  const activeRef = useRef<HTMLSpanElement>(null);
  useEffect(() => {
    activeRef.current?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [activeId]);
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
        {/* Owns the window's top-right corner whenever the panel is open, so it
            needs its own drag region for the same reason the hero banner does. */}
        <div className={styles.aux_head} data-tauri-drag-region>
          <div className={styles.aux_tabs} role="tablist" data-tauri-drag-region>
            {tabs.map((tab) => (
              <span
                key={tab.id}
                ref={activeId === tab.id ? activeRef : undefined}
                className={`${styles.aux_tab} ${activeId === tab.id ? styles.aux_tab_active : ""}`}
              >
                <button
                  type="button"
                  role="tab"
                  aria-selected={activeId === tab.id}
                  className={styles.aux_tab_label}
                  onClick={() => onPick(tab.id)}
                >
                  {tab.label}
                </button>
                {tab.closable && (
                  <button
                    type="button"
                    className={styles.aux_tab_close}
                    onClick={() => onCloseTab(tab.id)}
                    aria-label={t("common.close", "关闭")}
                  >
                    ✕
                  </button>
                )}
              </span>
            ))}
          </div>
          <button
            type="button"
            className={styles.aux_close}
            onClick={onClose}
            title={t("detail.aux_hide", "收起辅助栏")}
            aria-label={t("detail.aux_hide", "收起辅助栏")}
          >
            ✕
          </button>
        </div>
        <div className={styles.aux_body}>{children}</div>
      </aside>
    </>
  );
}
