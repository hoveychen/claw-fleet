import { useCallback, useEffect, useState, type PointerEvent as ReactPointerEvent, type RefObject } from "react";
import { getItem, setItem } from "../storage";

/** The expanded doc card's width, as a fraction of the pane it sits in.
 *
 *  A *ratio*, not a pixel count, because the same app runs on a 13" laptop and
 *  on a 3440px ultrawide: a width that reads as "half the conversation" on one
 *  is a hairline column or a full-screen takeover on the other. The reader's
 *  drag is therefore stored as a proportion and re-resolved against whatever
 *  pane it lands in. Registered in storage.ts's ALL_KEYS. */
const RATIO_KEY = "detail-doc-card-ratio";
export const RATIO_DEFAULT = 0.46;
const RATIO_MIN = 0.25;
/** The conversation keeps at least this share of the pane. The card is what the
 *  reader dragged, but a card that can eat the transcript whole is the bug this
 *  whole change came from. */
const RATIO_MAX = 0.72;
/** Floor in px, so the ratio cannot squeeze the reader below something a file
 *  path or a web page can render in. On a pane too narrow to give both the
 *  floor and the transcript's share, the pane's share wins — see below. */
const CARD_MIN_PX = 300;

function clampRatio(r: number): number {
  return Math.min(RATIO_MAX, Math.max(RATIO_MIN, r));
}

/**
 * Width (px) of the expanded doc card, plus the grip that resizes it.
 *
 * Lives here rather than in `SessionAuxRail` because two elements need the
 * number: the card itself, and the conversation, which holds a band of that
 * width clear so the card never covers the prose. That band is applied as
 * padding *inside* the transcript's scroller (see `--rail-band`), so the
 * scrollbar stays on the pane's own right edge.
 *
 * `paneRef` is the box the ratio resolves against — the messages pane.
 */
export function useDocCardWidth(paneRef: RefObject<HTMLElement | null>, enabled: boolean) {
  const [ratio, setRatio] = useState(() => {
    const saved = getItem(RATIO_KEY);
    if (saved) {
      const n = Number.parseFloat(saved);
      if (Number.isFinite(n)) return clampRatio(n);
    }
    return RATIO_DEFAULT;
  });
  // Measured rather than assumed: it changes with the window, the sidebar, and
  // the 任务 page's 4-way split.
  const [paneW, setPaneW] = useState(0);

  useEffect(() => {
    const pane = paneRef.current;
    if (!pane) return;
    const ro = new ResizeObserver(() => setPaneW(pane.clientWidth));
    ro.observe(pane);
    setPaneW(pane.clientWidth);
    return () => ro.disconnect();
  }, [paneRef, enabled]);

  const onGripDown = useCallback(
    (e: ReactPointerEvent<HTMLElement>) => {
      const pane = paneRef.current;
      if (!pane) return;
      e.preventDefault();
      const rect = pane.getBoundingClientRect();
      const handle = e.currentTarget;
      handle.setPointerCapture(e.pointerId);
      const move = (ev: PointerEvent) => {
        // The card hugs the pane's right edge, so its width is just "how far
        // the cursor is from that edge" — no start offset to remember.
        setRatio(clampRatio((rect.right - ev.clientX) / Math.max(1, rect.width)));
      };
      const up = () => {
        handle.releasePointerCapture(e.pointerId);
        handle.removeEventListener("pointermove", move);
        handle.removeEventListener("pointerup", up);
        setRatio((r) => {
          setItem(RATIO_KEY, r.toFixed(3));
          return r;
        });
      };
      handle.addEventListener("pointermove", move);
      handle.addEventListener("pointerup", up);
    },
    [paneRef],
  );

  // The floor wins over the ratio, and the pane wins over the floor: on a pane
  // too narrow for both, there is no width that satisfies everyone and the card
  // takes what there is rather than overflowing.
  const avail = Math.max(0, paneW - 20);
  const width = enabled
    ? Math.round(Math.min(avail, Math.max(Math.min(CARD_MIN_PX, avail), ratio * paneW)))
    : 0;

  return { width, paneW, onGripDown };
}
