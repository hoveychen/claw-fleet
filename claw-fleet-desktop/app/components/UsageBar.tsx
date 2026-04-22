import { useState } from "react";
import styles from "./UsageBar.module.css";

export interface UsageBarSource {
  name: string;
  percent: number;
}

export interface UsageBarData {
  percent: number;
  topSource?: string;
  sources?: UsageBarSource[];
  onClick?: () => void;
}

function colorFor(pct: number): string {
  if (pct >= 85) return "#ef4444";
  if (pct >= 70) return "#fbbf24";
  return "#4ade80";
}

export function UsageBar({ data }: { data: UsageBarData }) {
  const [hovered, setHovered] = useState(false);
  const pct = Math.max(0, Math.min(100, data.percent));
  const color = colorFor(pct);
  const sources = (data.sources ?? []).slice().sort((a, b) => b.percent - a.percent);
  const clickable = typeof data.onClick === "function";
  const showBreakdown = !clickable && hovered && sources.length > 0;

  return (
    <div
      className={`${styles.wrap} ${clickable ? styles.clickable : ""}`}
      onMouseEnter={() => setHovered(true)}
      onMouseLeave={() => setHovered(false)}
      onClick={clickable ? (e) => { e.stopPropagation(); data.onClick?.(); } : undefined}
      role={clickable ? "button" : undefined}
      tabIndex={clickable ? 0 : undefined}
      onKeyDown={clickable ? (e) => { if (e.key === "Enter" || e.key === " ") { e.preventDefault(); data.onClick?.(); } } : undefined}
    >
      <div className={styles.row}>
        <span className={styles.label}>{data.topSource ?? "Usage"}</span>
        <span className={styles.track}>
          <span
            className={styles.fill}
            style={{ width: `${pct}%`, background: color }}
          />
        </span>
        <span className={styles.pct} style={{ color }}>{Math.round(pct)}%</span>
      </div>
      {showBreakdown && (
        <div className={styles.popover}>
          {sources.map((s) => {
            const p = Math.max(0, Math.min(100, s.percent));
            const c = colorFor(p);
            return (
              <div key={s.name} className={styles.row}>
                <span className={styles.label}>{s.name}</span>
                <span className={styles.track}>
                  <span className={styles.fill} style={{ width: `${p}%`, background: c }} />
                </span>
                <span className={styles.pct} style={{ color: c }}>{Math.round(p)}%</span>
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
