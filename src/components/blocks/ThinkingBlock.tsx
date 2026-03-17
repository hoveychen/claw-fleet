import { useState } from "react";
import styles from "./ThinkingBlock.module.css";

interface Props {
  thinking: string;
}

export function ThinkingBlock({ thinking }: Props) {
  const [open, setOpen] = useState(false);
  const preview = thinking.slice(0, 80).replace(/\n/g, " ");

  return (
    <div className={styles.root}>
      <button className={styles.toggle} onClick={() => setOpen((o) => !o)}>
        <span className={styles.icon}>{open ? "▾" : "▸"}</span>
        <span className={styles.label}>Thinking</span>
        {!open && (
          <span className={styles.preview}>
            {preview}
            {thinking.length > 80 ? "…" : ""}
          </span>
        )}
      </button>
      {open && <pre className={styles.content}>{thinking}</pre>}
    </div>
  );
}
