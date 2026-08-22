import { useRef } from "react";
import type { SplitOrientation } from "../tabGroups";
import styles from "./GroupDivider.module.css";

/**
 * Draggable boundary between two adjacent editor groups.
 *
 * Reports the pointer's movement *since the last event* rather than since the
 * press, so the host can apply it to the live weights without holding a
 * snapshot — the clamping in `resizeGroups` is per-pixel and idempotent, so
 * feeding it increments lands in the same place as feeding it a total.
 *
 * Pointer capture (rather than document-level mousemove listeners) is what keeps
 * the drag alive when the cursor crosses into a pane and over an iframe: the
 * events keep coming to this element until release.
 */
export function GroupDivider({
  orientation,
  onResize,
}: {
  orientation: SplitOrientation;
  /** Pixels the boundary moved since the previous event — positive means
   *  right/down, growing the group before it. */
  onResize: (deltaPx: number) => void;
}) {
  const last = useRef(0);
  const vertical = orientation === "row";
  return (
    <div
      className={`${styles.divider} ${vertical ? styles.divider_v : styles.divider_h}`}
      role="separator"
      // A vertical *bar* separates groups laid out in a row; ARIA names the
      // separator's own orientation, which is the opposite of the axis.
      aria-orientation={vertical ? "vertical" : "horizontal"}
      onPointerDown={(e) => {
        e.preventDefault();
        e.currentTarget.setPointerCapture(e.pointerId);
        last.current = vertical ? e.clientX : e.clientY;
      }}
      onPointerMove={(e) => {
        if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
        const pos = vertical ? e.clientX : e.clientY;
        const delta = pos - last.current;
        if (delta === 0) return;
        last.current = pos;
        onResize(delta);
      }}
      onPointerUp={(e) => e.currentTarget.releasePointerCapture(e.pointerId)}
      onPointerCancel={(e) => e.currentTarget.releasePointerCapture(e.pointerId)}
    />
  );
}
