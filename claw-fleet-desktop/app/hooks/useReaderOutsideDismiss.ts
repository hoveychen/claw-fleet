// Click anywhere outside an expanded rail reader to collapse it.
//
// While a reader is open, useChromeYield folds the nav's session list away (and
// the rail may fold too) to make room. The way back to the list is a small
// button in the nav footer, and people did not find it — the reader looked like
// a page they were stuck on. Collapsing it on an outside click brings the
// yielded chrome back by itself.
//
// What counts as "outside":
// - inside the app root (`#root`), so overlays portaled to <body> — context
//   menus, the maximized reader, lightboxes, modals — never dismiss it;
// - not inside the reader host or any `[data-reader-keep]` element (the rail,
//   whose own rows toggle the reader, and the header's rail switch);
// - a click, not a drag: a pointer that travelled or left a text selection was
//   selecting prose, which the reader is there to be read beside.

import { useEffect } from "react";

const DRAG_SLOP_PX = 4;

export function isReaderOutsideTarget(
  target: EventTarget | null,
  readerHost: HTMLElement | null,
  appRoot: HTMLElement | null,
): boolean {
  if (!(target instanceof Node)) return false;
  if (!appRoot || !appRoot.contains(target)) return false;
  if (readerHost?.contains(target)) return false;
  const el = target instanceof Element ? target : target.parentElement;
  if (el?.closest("[data-reader-keep]")) return false;
  return true;
}

export function useReaderOutsideDismiss(
  active: boolean,
  readerHost: HTMLElement | null,
  onDismiss: () => void,
): void {
  useEffect(() => {
    if (!active) return;
    let down: { x: number; y: number; outside: boolean } | null = null;
    const onDown = (e: PointerEvent) => {
      down =
        e.button === 0
          ? {
              x: e.clientX,
              y: e.clientY,
              outside: isReaderOutsideTarget(e.target, readerHost, document.getElementById("root")),
            }
          : null;
    };
    const onUp = (e: PointerEvent) => {
      const start = down;
      down = null;
      if (!start?.outside || e.button !== 0) return;
      if (Math.hypot(e.clientX - start.x, e.clientY - start.y) > DRAG_SLOP_PX) return;
      const sel = window.getSelection();
      if (sel && !sel.isCollapsed) return;
      onDismiss();
    };
    // Capture phase: a handler that stops propagation on its own element must
    // not hide the click from us.
    window.addEventListener("pointerdown", onDown, true);
    window.addEventListener("pointerup", onUp, true);
    return () => {
      window.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("pointerup", onUp, true);
    };
  }, [active, readerHost, onDismiss]);
}
