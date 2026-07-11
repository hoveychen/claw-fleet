import { useCallback, useRef, useState } from "react";
import { getItem, setItem } from "../storage";

type Options = {
  min: number;
  max: number;
  initial: number;
  /** Which edge the handle sits on. A right-side pane grows as the cursor
   *  moves left, so its drag delta is inverted. */
  side?: "left" | "right";
};

/**
 * Drag-to-resize width for a pane, persisted across restarts.
 *
 * `storageKey` must be listed in storage.ts's ALL_KEYS, otherwise the width is
 * written but never read back on boot.
 */
export function useResizableWidth(storageKey: string, { min, max, initial, side = "left" }: Options) {
  const [width, setWidth] = useState(() => {
    const saved = getItem(storageKey);
    if (saved) {
      const n = parseInt(saved, 10);
      if (n >= min && n <= max) return n;
    }
    return initial;
  });
  const [isDragging, setIsDragging] = useState(false);
  const dragRef = useRef<{ startX: number; startWidth: number } | null>(null);

  const onMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault();
      dragRef.current = { startX: e.clientX, startWidth: width };
      setIsDragging(true);

      function onMouseMove(e: MouseEvent) {
        if (!dragRef.current) return;
        const travel = e.clientX - dragRef.current.startX;
        const delta = side === "right" ? -travel : travel;
        setWidth(Math.min(max, Math.max(min, dragRef.current.startWidth + delta)));
      }

      function onMouseUp() {
        dragRef.current = null;
        setIsDragging(false);
        setWidth((prev) => {
          setItem(storageKey, String(prev));
          return prev;
        });
        document.removeEventListener("mousemove", onMouseMove);
        document.removeEventListener("mouseup", onMouseUp);
      }

      document.addEventListener("mousemove", onMouseMove);
      document.addEventListener("mouseup", onMouseUp);
    },
    [width, storageKey, min, max, side],
  );

  return { width, isDragging, onMouseDown };
}
