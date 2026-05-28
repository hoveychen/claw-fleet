import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import styles from "./WindowsFrameOverlay.module.css";

type ResizeDirection =
  | "North"
  | "South"
  | "East"
  | "West"
  | "NorthEast"
  | "NorthWest"
  | "SouthEast"
  | "SouthWest";

const RESIZE_HANDLES: { dir: ResizeDirection; className: string }[] = [
  { dir: "North", className: "edge_n" },
  { dir: "South", className: "edge_s" },
  { dir: "East", className: "edge_e" },
  { dir: "West", className: "edge_w" },
  { dir: "NorthWest", className: "corner_nw" },
  { dir: "NorthEast", className: "corner_ne" },
  { dir: "SouthWest", className: "corner_sw" },
  { dir: "SouthEast", className: "corner_se" },
];

// Inline 10x10 SVGs so stroke width and glyph proportion match Windows
// native caption buttons. Using a shared 16x16 Icon component would shrink
// the stroke to ~0.6px (frameless skill Rule 6a).
const CaptionMinimize = () => (
  <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1" strokeLinecap="square" aria-hidden>
    <path d="M0.5 5h9" />
  </svg>
);

const CaptionMaximize = () => (
  <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1" strokeLinecap="square" aria-hidden>
    <rect x="0.5" y="0.5" width="9" height="9" />
  </svg>
);

const CaptionRestore = () => (
  <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1" strokeLinecap="square" aria-hidden>
    <rect x="0.5" y="2.5" width="7" height="7" />
    <path d="M2.5 2.5V0.5h7v7H7.5" />
  </svg>
);

const CaptionClose = () => (
  <svg width="10" height="10" viewBox="0 0 10 10" fill="none" stroke="currentColor" strokeWidth="1" strokeLinecap="square" aria-hidden>
    <path d="M0.5 0.5l9 9 M9.5 0.5l-9 9" />
  </svg>
);

export function WindowsFrameOverlay() {
  const [isMaximized, setIsMaximized] = useState(false);

  useEffect(() => {
    const w = getCurrentWindow();
    let cancelled = false;
    w.isMaximized()
      .then((v) => { if (!cancelled) setIsMaximized(!!v); })
      .catch(() => {});
    const unlistenPromise = w.onResized(() => {
      w.isMaximized()
        .then((v) => { if (!cancelled) setIsMaximized(!!v); })
        .catch(() => {});
    });
    return () => {
      cancelled = true;
      unlistenPromise.then((fn) => fn()).catch(() => {});
    };
  }, []);

  const onMinimize = () => { getCurrentWindow().minimize().catch(() => {}); };
  const onToggleMax = () => { getCurrentWindow().toggleMaximize().catch(() => {}); };
  const onClose = () => { getCurrentWindow().close().catch(() => {}); };

  const onResizeStart = (dir: ResizeDirection) => (e: React.MouseEvent) => {
    e.preventDefault();
    getCurrentWindow().startResizeDragging(dir).catch(() => {});
  };

  return (
    <>
      {/* Invisible drag strip across the top. macOS uses it to provide the
          OS-level drag region (since titleBarStyle: Overlay leaves a
          transparent chrome zone over the traffic lights); Windows uses it
          as a fallback drag region for the empty area between caption
          buttons and the app content. */}
      <div className={styles.drag_bar} data-tauri-drag-region />

      {/* Caption buttons — Windows only. CSS hides them on macOS so the
          same component can stay mounted everywhere. */}
      <div className={styles.caption_group}>
        <button
          type="button"
          className={styles.caption_btn}
          aria-label="Minimize"
          title="Minimize"
          onClick={onMinimize}
        >
          <CaptionMinimize />
        </button>
        <button
          type="button"
          className={styles.caption_btn}
          aria-label={isMaximized ? "Restore" : "Maximize"}
          title={isMaximized ? "Restore" : "Maximize"}
          onClick={onToggleMax}
        >
          {isMaximized ? <CaptionRestore /> : <CaptionMaximize />}
        </button>
        <button
          type="button"
          className={`${styles.caption_btn} ${styles.caption_close}`}
          aria-label="Close"
          title="Close"
          onClick={onClose}
        >
          <CaptionClose />
        </button>
      </div>

      {/* Resize handles — Windows only, suppressed while maximized since
          a maximized window has no resize border on the real OS.
          decorations:false strips the native resize border, so mousedown
          hands the OS a manual resize intent via startResizeDragging. */}
      {!isMaximized && (
        <div className={styles.resize_layer} aria-hidden>
          {RESIZE_HANDLES.map(({ dir, className }) => (
            <div
              key={dir}
              className={`${styles.resize_handle} ${styles[className]}`}
              onMouseDown={onResizeStart(dir)}
            />
          ))}
        </div>
      )}
    </>
  );
}
