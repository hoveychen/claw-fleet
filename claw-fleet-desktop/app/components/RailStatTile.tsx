import styles from "./RailStatTile.module.css";

interface Props {
  /** Already-formatted number — use `fmtRailMoney` / `fmtRailCount` so it fits. */
  value: string;
  /** Unit or caption under the value. Truncates with an ellipsis. */
  label: string;
  /** Full-precision tooltip. Always supply one: the tile itself is lossy. */
  title?: string;
  /** Present → the tile renders as a <button>. */
  onClick?: () => void;
  /** Forwarded to the root so the onboarding wizard can still target it. */
  dataWizard?: string;
  /** Paint the value in the accent color (used by the usage-percent tile). */
  accent?: boolean;
}

/** One number tile in the 64px collapsed sidebar rail: a big value over a
 *  small caption. Three separate panels used to each carry their own copy of
 *  this box, which is why the collapsed stack rendered at three different
 *  value sizes with three different paddings — they all share this now, so
 *  spacing and typography stay identical no matter which tiles are visible. */
export function RailStatTile({ value, label, title, onClick, dataWizard, accent }: Props) {
  const className = `${styles.tile}${accent ? ` ${styles.tile_accent}` : ""}`;
  const body = (
    <>
      <span className={styles.value}>{value}</span>
      <span className={styles.label}>{label}</span>
    </>
  );
  if (onClick) {
    return (
      <button
        type="button"
        className={`${className} ${styles.clickable}`}
        title={title}
        onClick={onClick}
        data-wizard={dataWizard}
      >
        {body}
      </button>
    );
  }
  return (
    <div className={className} title={title} data-wizard={dataWizard}>
      {body}
    </div>
  );
}
