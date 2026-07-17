import { useEffect, useRef, useState } from "react";
import { TextBlock } from "./TextBlock";
import styles from "./ThinkingBlock.module.css";

interface Props {
  thinking: string;
  /** Live sidecar stream (still being written): the reasoning scrolls behind a
   *  fixed-height window pinned to the newest text, with the oldest lines
   *  fading out at the top — the claude.ai streaming-thinking look. */
  live?: boolean;
  /** Rendered as a work-run rail step: the gutter icon already names the
   *  block, so skip the "Thinking" header row and show the text directly,
   *  clamped to a window with a bottom fade. Click toggles the full text. */
  rail?: boolean;
}

/** Measures whether the clamped window actually overflows — the fade is only
 *  drawn when there is hidden text for it to hint at, so short reasoning
 *  doesn't get its edge lines washed out for nothing. */
function useOverflow(ref: React.RefObject<HTMLDivElement | null>, deps: unknown[]) {
  const [overflowing, setOverflowing] = useState(false);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    setOverflowing(el.scrollHeight > el.clientHeight + 1);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, deps);
  return overflowing;
}

export function ThinkingBlock({ thinking, live = false, rail = false }: Props) {
  const [open, setOpen] = useState(live);
  const windowRef = useRef<HTMLDivElement>(null);
  const overflowing = useOverflow(windowRef, [thinking, open, live, rail]);

  // Live stream: keep the window pinned to the newest text as it arrives.
  useEffect(() => {
    if (!live) return;
    const el = windowRef.current;
    if (el) el.scrollTop = el.scrollHeight;
  }, [live, thinking]);

  if (rail && !live) {
    const clamped = !open;
    return (
      <div
        className={`${styles.rail_window} ${clamped ? styles.rail_clamped : ""} ${
          clamped && overflowing ? styles.fade_bottom : ""
        }`}
        ref={windowRef}
        onClick={() => setOpen((o) => !o)}
      >
        <TextBlock text={thinking} />
      </div>
    );
  }

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
      {open && (
        <div
          ref={windowRef}
          className={`${styles.content} ${live ? styles.live_window : ""} ${
            live && overflowing ? styles.fade_top : ""
          }`}
        >
          <TextBlock text={thinking} />
        </div>
      )}
    </div>
  );
}
