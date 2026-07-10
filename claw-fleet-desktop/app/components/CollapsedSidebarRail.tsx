import { useTranslation } from "react-i18next";
import styles from "./CollapsedSidebarRail.module.css";

interface Props {
  /** Expand the sidebar (called on click). */
  onExpand: () => void;
  /** Which edge the rail hugs. Determines the border side and the direction
   *  the expand chevron points. Defaults to "left". */
  side?: "left" | "right";
  /** Optional tooltip / aria-label override. Defaults to a generic
   *  "expand sidebar" string. */
  title?: string;
}

/** A thin, full-height clickable strip that replaces a collapsed secondary
 *  sidebar (二级侧边栏). It shows a panel-open glyph so it's visually obvious
 *  the pane can be expanded again — click it (or re-click the active nav item)
 *  to restore the sidebar. Shared by every view that owns a secondary sidebar
 *  so the affordance looks identical everywhere, regardless of each view's own
 *  CSS module. */
export function CollapsedSidebarRail({ onExpand, side = "left", title }: Props) {
  const { t } = useTranslation();
  const label = title ?? t("sidebar.secondary_expand", "展开侧边栏");
  // Left-hugging rail expands rightward → chevron points right; right-hugging
  // rail expands leftward → chevron points left.
  const chevron =
    side === "right" ? (
      <polyline points="9.5 5 6 8 9.5 11" />
    ) : (
      <polyline points="6.5 5 10 8 6.5 11" />
    );
  return (
    <button
      type="button"
      className={`${styles.rail} ${side === "right" ? styles.rail_right : styles.rail_left}`}
      onClick={onExpand}
      title={label}
      aria-label={label}
    >
      <span className={styles.icon}>
        <svg
          viewBox="0 0 16 16"
          width="16"
          height="16"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.3"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <rect x="1.5" y="2.5" width="13" height="11" rx="1.5" />
          <line x1="6" y1="2.5" x2="6" y2="13.5" />
          {chevron}
        </svg>
      </span>
    </button>
  );
}
