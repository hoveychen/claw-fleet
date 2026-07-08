import { useState } from "react";
import styles from "./ThinkingBlock.module.css";

interface Props {
  thinking: string;
  /** Live sidecar stream (still being written): default-open with a
   *  "Thinking…" label so the reasoning streams in as it arrives. */
  live?: boolean;
}

export function ThinkingBlock({ thinking, live = false }: Props) {
  const [open, setOpen] = useState(live);
  const preview = thinking.slice(0, 80).replace(/\n/g, " ");

  return (
    <div className={styles.root} data-live={live || undefined}>
      <button className={styles.toggle} onClick={() => setOpen((o) => !o)}>
        <span className={styles.icon}>{open ? "▾" : "▸"}</span>
        <span className={styles.label}>{live ? "Thinking…" : "Thinking"}</span>
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
