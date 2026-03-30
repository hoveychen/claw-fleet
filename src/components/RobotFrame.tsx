/**
 * RobotFrame — shared CRT-style robot container for mascot eyes.
 *
 * Used by both the sidebar (MascotEyes) and the floating overlay (OverlayMascot).
 * Renders a rounded frame with a "screen" area that has CRT scanlines,
 * vignette, and barrel distortion effects.
 *
 * - `children` is rendered inside the screen area
 * - `footer` is rendered below the screen (e.g. status bar in overlay)
 * - Forwards className, onClick, onDoubleClick, onContextMenu to the frame
 * - When `tauriDrag` is set, the entire frame becomes a drag region:
 *   mousedown + move → window drag; mousedown + release → normal click
 */

import { useCallback, useEffect, useRef, type ReactNode, type MouseEvent } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import styles from "./RobotFrame.module.css";

const DRAG_THRESHOLD = 4; // px before a press becomes a drag

export interface RobotFrameProps {
  children: ReactNode;
  footer?: ReactNode;
  className?: string;
  /** Make the frame a Tauri drag region (for floating overlay window). */
  tauriDrag?: boolean;
  onClick?: (e: MouseEvent) => void;
  onDoubleClick?: (e: MouseEvent) => void;
  onContextMenu?: (e: MouseEvent) => void;
}

export function RobotFrame({
  children,
  footer,
  className,
  tauriDrag,
  onClick,
  onDoubleClick,
  onContextMenu,
}: RobotFrameProps) {
  const frameClass = className ? `${styles.frame} ${className}` : styles.frame;
  const frameRef = useRef<HTMLDivElement>(null);
  const didDrag = useRef(false);

  // Capture-phase click listener: suppress click (including on child buttons)
  // after a drag gesture, before any React handler sees it.
  useEffect(() => {
    if (!tauriDrag) return;
    const el = frameRef.current;
    if (!el) return;

    const suppress = (e: Event) => {
      if (didDrag.current) {
        e.stopPropagation();
        e.preventDefault();
        didDrag.current = false;
      }
    };

    el.addEventListener("click", suppress, true); // capture phase
    return () => el.removeEventListener("click", suppress, true);
  }, [tauriDrag]);

  const handleMouseDown = useCallback(
    (e: MouseEvent) => {
      if (!tauriDrag || e.button !== 0) return;
      didDrag.current = false;
      const startX = e.screenX;
      const startY = e.screenY;

      const onMove = (me: globalThis.MouseEvent) => {
        const dx = me.screenX - startX;
        const dy = me.screenY - startY;
        if (dx * dx + dy * dy > DRAG_THRESHOLD * DRAG_THRESHOLD) {
          didDrag.current = true;
          cleanup();
          getCurrentWindow().startDragging().catch(() => {});
        }
      };

      const onUp = () => cleanup();

      const cleanup = () => {
        window.removeEventListener("mousemove", onMove);
        window.removeEventListener("mouseup", onUp);
      };

      window.addEventListener("mousemove", onMove);
      window.addEventListener("mouseup", onUp);
    },
    [tauriDrag],
  );

  return (
    <div
      ref={frameRef}
      className={frameClass}
      onMouseDown={handleMouseDown}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
      onContextMenu={onContextMenu}
    >
      <div className={styles.screen}>{children}</div>
      {footer}
    </div>
  );
}
