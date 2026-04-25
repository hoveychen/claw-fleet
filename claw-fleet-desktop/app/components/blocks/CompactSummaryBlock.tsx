import { useState } from "react";
import { TextBlock } from "./TextBlock";
import styles from "./CompactSummaryBlock.module.css";

interface Props {
  summary: string;
}

function formatSize(n: number): string {
  if (n < 1000) return `${n} chars`;
  if (n < 1_000_000) return `${(n / 1000).toFixed(1)}k chars`;
  return `${(n / 1_000_000).toFixed(2)}M chars`;
}

export function CompactSummaryBlock({ summary }: Props) {
  const [open, setOpen] = useState(false);
  return (
    <div className={styles.root}>
      <button className={styles.header} onClick={() => setOpen((o) => !o)}>
        <span className={styles.line} />
        <span className={styles.label}>
          <span className={styles.icon}>⇲</span>
          上下文已压缩
          <span className={styles.size}>· {formatSize(summary.length)}</span>
          <span className={styles.arrow}>{open ? "▾" : "▸"}</span>
        </span>
        <span className={styles.line} />
      </button>
      {open && (
        <div className={styles.body}>
          <TextBlock text={summary} />
        </div>
      )}
    </div>
  );
}
