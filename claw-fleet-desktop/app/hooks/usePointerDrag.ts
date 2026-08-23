// Drag-and-drop built on pointer events instead of the HTML5 drag API.
//
// WHY NOT HTML5 DnD: it is dead inside this app's webview. Tauri registers a
// drag-drop handler on the webview whenever `dragDropEnabled` is on (it is, by
// default, and this app needs it — ChatComposer reads dropped file *paths* off
// `onDragDropEvent`). wry's macOS implementation only forwards the AppKit
// dragging-destination callbacks to WebKit when that handler declines the drag,
// and Tauri's handler never declines: it returns `true` unconditionally
// (tauri-runtime-wry/src/lib.rs, `with_drag_drop_handler`). So
// `draggingEntered:` / `draggingUpdated:` / `performDragOperation:` never reach
// super, and the page sees `dragstart` … `dragend` with no `dragenter`,
// `dragover` or `drop` in between — measured on a minimal Tauri 2.11 / wry 0.55
// app driven by real mouse events, which logged dragstart=1, dragend=1,
// dragover=0, drop=0. Any feature built on `onDrop` is therefore inert in the
// desktop app while working perfectly in a browser, which is exactly how the
// tab strip's cross-group move shipped broken.
//
// Pointer events have no such interception: they are ordinary DOM input events.
// The cost is that nothing is dropped *for* us — the drop target is resolved by
// hit-testing `document.elementFromPoint` on every move, which is what
// `dropTargetAt` is for.

import { useCallback, useRef } from "react";

/** Pixels the pointer must travel before a press becomes a drag. Below this a
 *  press is still a click, so tabs/cards stay clickable. */
export const DRAG_THRESHOLD_PX = 5;

export interface PointerDragPoint {
  x: number;
  y: number;
  /** Topmost element under the pointer, or null when outside the window.
   *  Resolved by hit-test rather than event target: pointer capture keeps every
   *  event aimed at the element the drag started on. */
  over: Element | null;
}

export interface PointerDragSpec {
  /** Fires once, when the press first travels past the threshold. Return false
   *  to refuse the drag (it then stays a plain click). */
  onStart?: (p: PointerDragPoint) => boolean | void;
  onMove?: (p: PointerDragPoint) => void;
  /** Pointer released after a drag actually started. */
  onDrop?: (p: PointerDragPoint) => void;
  /** The drag was abandoned: Escape, pointercancel, or the browser taking the
   *  pointer away. Never fires for a press that stayed under the threshold. */
  onCancel?: () => void;
}

/**
 * Returns an `onPointerDown` handler that turns a press into a drag once the
 * pointer travels `DRAG_THRESHOLD_PX`, plus a `didDrag()` probe callers use to
 * swallow the click that a completed drag would otherwise fire.
 *
 * The spec is read through a ref, so callers can pass fresh closures every
 * render without restarting or staling an in-flight drag.
 */
export function usePointerDrag(spec: PointerDragSpec) {
  const specRef = useRef(spec);
  specRef.current = spec;
  // Set on drop/cancel, cleared by the first `didDrag()` read. The click that
  // follows a drag would otherwise activate whatever the press landed on.
  const draggedRef = useRef(false);

  const onPointerDown = useCallback((e: React.PointerEvent) => {
    // Only the primary button drags; right-click opens the context menu and
    // middle-click closes the tab.
    if (e.button !== 0) return;
    const el = e.currentTarget as HTMLElement;
    const startX = e.clientX;
    const startY = e.clientY;
    const pointerId = e.pointerId;
    let started = false;

    const point = (ev: PointerEvent): PointerDragPoint => ({
      x: ev.clientX,
      y: ev.clientY,
      over: document.elementFromPoint(ev.clientX, ev.clientY),
    });

    const finish = (dropped: PointerDragPoint | null) => {
      // Listeners live on `window`, not on `el`: before the threshold is crossed
      // there is no pointer capture yet, so a fast flick out of the element
      // would otherwise stop delivering moves and the drag would never start.
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("keydown", onKey, true);
      if (el.hasPointerCapture(pointerId)) el.releasePointerCapture(pointerId);
      if (!started) return;
      document.body.style.userSelect = "";
      draggedRef.current = true;
      if (dropped) specRef.current.onDrop?.(dropped);
      else specRef.current.onCancel?.();
    };

    function onMove(ev: PointerEvent) {
      if (!started) {
        if (Math.abs(ev.clientX - startX) < DRAG_THRESHOLD_PX &&
            Math.abs(ev.clientY - startY) < DRAG_THRESHOLD_PX) return;
        // Capturing here rather than on pointerdown keeps a plain click from
        // ever being retargeted — and keeps the pointer stream on this element
        // for the rest of the drag, even over iframes and other panes.
        el.setPointerCapture(pointerId);
        started = true;
        if (specRef.current.onStart?.(point(ev)) === false) {
          started = false;
          finish(null);
          return;
        }
        // A drag that paints a text selection behind it reads as a bug.
        document.body.style.userSelect = "none";
      }
      specRef.current.onMove?.(point(ev));
    }
    function onUp(ev: PointerEvent) {
      finish(started ? point(ev) : null);
    }
    function onCancel() {
      finish(null);
    }
    function onKey(ev: KeyboardEvent) {
      if (ev.key !== "Escape" || !started) return;
      ev.preventDefault();
      finish(null);
    }

    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onKey, true);
  }, []);

  /** True exactly once after a drag ended, so the trailing click can be
   *  swallowed by the element the drag started on. */
  const didDrag = useCallback(() => {
    if (!draggedRef.current) return false;
    draggedRef.current = false;
    return true;
  }, []);

  return { onPointerDown, didDrag };
}

/**
 * Walk up from the hit-tested element to the nearest ancestor carrying
 * `attr`, and return its value. The pointer-drag equivalent of a drop target:
 * mark drop zones with a data attribute and read it back here.
 */
export function dropTargetAt(over: Element | null, attr: string): string | null {
  const el = over?.closest(`[${attr}]`);
  return el ? el.getAttribute(attr) : null;
}
