// Component-level right-click menu.
//
// The app-wide menu in `contextMenu.ts` (Settings / About / Quit) backs off
// whenever a handler calls `preventDefault()`, which is what an anchor using
// this component must do. Rendered through a portal because the menu's callers
// live inside `overflow: auto` panes that would otherwise clip it.

import { useEffect, useLayoutEffect, useRef, useState, type ReactNode } from "react";
import { createPortal } from "react-dom";
import styles from "./ContextMenu.module.css";

export interface ContextMenuItem {
  id: string;
  label: string;
  icon?: ReactNode;
  /** Dim monospace second line — the value the item acts on (a path, an id). */
  sub?: string;
  /** Renders in the danger colour and sits below a separator. */
  danger?: boolean;
  /** Marks the item as the one currently in effect — the facet the auxiliary
   *  panel is showing, say. Rendered as a trailing dot, not a checkbox: it is a
   *  "you are here", not a setting you toggled. */
  active?: boolean;
  /** Draws a separator above this item, so one menu can hold two families
   *  (things that change the layout vs. things that hand you a string). */
  dividerBefore?: boolean;
  onSelect: () => void;
}

export interface ContextMenuAnchor {
  x: number;
  y: number;
}

const VIEWPORT_PAD = 4;

export function ContextMenu({
  anchor,
  items,
  onClose,
}: {
  anchor: ContextMenuAnchor;
  items: ContextMenuItem[];
  onClose: () => void;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState(anchor);

  // Measure before paint so the menu never flashes off-screen.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const { width, height } = el.getBoundingClientRect();
    setPos({
      x:
        anchor.x + width > window.innerWidth - VIEWPORT_PAD
          ? Math.max(VIEWPORT_PAD, window.innerWidth - width - VIEWPORT_PAD)
          : anchor.x,
      y:
        anchor.y + height > window.innerHeight - VIEWPORT_PAD
          ? Math.max(VIEWPORT_PAD, window.innerHeight - height - VIEWPORT_PAD)
          : anchor.y,
    });
  }, [anchor.x, anchor.y]);

  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    // Capture phase: a card's own mousedown must not win the race to select.
    document.addEventListener("mousedown", onDown, true);
    document.addEventListener("keydown", onKey, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("resize", onClose);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
      document.removeEventListener("keydown", onKey, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("resize", onClose);
    };
  }, [onClose]);

  const normal = items.filter((i) => !i.danger);
  const danger = items.filter((i) => i.danger);

  const renderItem = (item: ContextMenuItem) => {
    const button = (
      <button
        key={item.id}
        className={`${styles.item} ${item.danger ? styles.item_danger : ""} ${item.active ? styles.item_active : ""}`}
        aria-current={item.active ? "true" : undefined}
        onClick={() => {
          onClose();
          item.onSelect();
        }}
      >
        <span className={styles.item_icon}>{item.icon}</span>
        {item.sub ? (
          <span className={styles.item_text}>
            <span>{item.label}</span>
            <span className={styles.item_sub} title={item.sub}>
              {item.sub}
            </span>
          </span>
        ) : (
          <span>{item.label}</span>
        )}
        {item.active && <span className={styles.item_dot} aria-hidden="true" />}
      </button>
    );
    if (!item.dividerBefore) return button;
    return (
      <div key={`${item.id}-group`} className={styles.item_group}>
        <div className={styles.separator} />
        {button}
      </div>
    );
  };

  return createPortal(
    <div ref={ref} className={styles.menu} style={{ left: pos.x, top: pos.y }}>
      {normal.map(renderItem)}
      {danger.length > 0 && normal.length > 0 && <div className={styles.separator} />}
      {danger.map(renderItem)}
    </div>,
    document.body,
  );
}

/**
 * Owns the open/closed state of one context menu. `open(event)` is a
 * `onContextMenu` handler: it suppresses the app-wide menu and pins the
 * component menu to the cursor.
 */
export function useContextMenu() {
  const [anchor, setAnchor] = useState<ContextMenuAnchor | null>(null);
  return {
    anchor,
    open: (e: React.MouseEvent) => {
      e.preventDefault();
      e.stopPropagation();
      setAnchor({ x: e.clientX, y: e.clientY });
    },
    close: () => setAnchor(null),
  };
}
