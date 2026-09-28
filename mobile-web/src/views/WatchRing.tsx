import styles from "./WatchRing.module.css";

/**
 * A watch's progress drawn as a ring, sized to stand in for the watch icon —
 * the chip gains progress without growing wider. Amber when `alarming`.
 */
export function WatchRing({
  fraction,
  alarming = false,
  size,
}: {
  fraction: number;
  alarming?: boolean;
  size: number;
}) {
  const pct = Math.round(Math.max(0, Math.min(1, fraction)) * 100);
  const r = 6;
  const c = 2 * Math.PI * r;
  return (
    <svg
      className={styles.ring}
      width={size}
      height={size}
      viewBox="0 0 16 16"
      role="progressbar"
      aria-valuenow={pct}
      aria-valuemin={0}
      aria-valuemax={100}
    >
      <circle className={styles.track} cx={8} cy={8} r={r} />
      <circle
        className={alarming ? styles.arc_alarming : styles.arc}
        cx={8}
        cy={8}
        r={r}
        strokeDasharray={`${(c * pct) / 100} ${c}`}
        transform="rotate(-90 8 8)"
      />
    </svg>
  );
}
