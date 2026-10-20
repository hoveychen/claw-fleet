import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent, type RefObject } from "react";

export const MIN_ZOOM = 1;
export const MAX_ZOOM = 5;

/**
 * macOS-Preview-style zoom and pan for an image inside a clipping stage.
 *
 * Pinch (WebKit gesture events, or Chromium's ctrl+wheel) and ⌘+wheel zoom
 * around the pointer; a two-finger scroll or a drag pans once zoomed; pan is
 * clamped so the image always covers the stage. Shared by the chat image
 * lightbox and the maximized reader card, so both answer the same gestures.
 *
 * With `enabled` false the hook attaches nothing and reports zoom 1, so a
 * stage that only sometimes zooms (a rail card that is not maximized) keeps
 * the page's normal wheel scrolling.
 */
export function useZoomPan(
  stageRef: RefObject<HTMLElement | null>,
  imgRef: RefObject<HTMLElement | null>,
  enabled = true,
) {
  const [zoom, setZoom] = useState(1);
  const [pan, setPan] = useState({ x: 0, y: 0 });
  const drag = useRef<{ x: number; y: number; panX: number; panY: number } | null>(null);
  // Latest zoom/pan for the native gesture listeners, which outlive renders.
  const view = useRef({ zoom: 1, pan: { x: 0, y: 0 } });

  // Keeps the scaled image covering the stage: no panning past its edges.
  const clampPan = (scale: number, x: number, y: number) => {
    const img = imgRef.current, stage = stageRef.current;
    if (!img || !stage) return { x, y };
    const limitX = Math.max(0, (img.offsetWidth * scale - stage.clientWidth) / 2);
    const limitY = Math.max(0, (img.offsetHeight * scale - stage.clientHeight) / 2);
    return { x: Math.max(-limitX, Math.min(limitX, x)), y: Math.max(-limitY, Math.min(limitY, y)) };
  };

  const applyView = (scale: number, x: number, y: number) => {
    const next = { zoom: scale, pan: clampPan(scale, x, y) };
    view.current = next;
    setZoom(next.zoom);
    setPan(next.pan);
  };

  // Zooms to `value`, keeping the image point under `anchor` (client coords;
  // the stage centre when omitted) fixed on screen, like macOS Preview.
  const setScale = (value: number, anchor?: { x: number; y: number }) => {
    const next = Math.max(MIN_ZOOM, Math.min(MAX_ZOOM, value));
    const { zoom: cur, pan: p } = view.current;
    const rect = stageRef.current?.getBoundingClientRect();
    if (!rect || !anchor || next === 1) { applyView(next, next === 1 ? 0 : p.x * next / cur, next === 1 ? 0 : p.y * next / cur); return; }
    const ax = anchor.x - (rect.left + rect.width / 2), ay = anchor.y - (rect.top + rect.height / 2);
    applyView(next, ax - (ax - p.x) * next / cur, ay - (ay - p.y) * next / cur);
  };

  // Leaving the zoomable state (the card was restored) drops back to fit.
  useEffect(() => {
    if (!enabled && view.current.zoom !== 1) applyView(1, 0, 0);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled]);

  // Trackpad gestures, macOS style: pinch zooms, two-finger scroll pans.
  // Native listeners because React's wheel handler is passive and cannot stop
  // the page from scrolling. WebKit reports a pinch as gesture* events; Chromium
  // (the web build) reports it as a wheel event with ctrlKey set.
  useEffect(() => {
    const stage = stageRef.current;
    if (!stage || !enabled) return;
    const onWheel = (e: WheelEvent) => {
      e.preventDefault();
      // ⌘ + wheel zooms too, for a plain mouse. A wheel notch reports a far
      // larger delta than a pinch frame, so cap the step.
      if (e.ctrlKey || e.metaKey) {
        const delta = Math.max(-25, Math.min(25, e.deltaY));
        setScale(view.current.zoom * Math.exp(-delta * 0.01), { x: e.clientX, y: e.clientY });
        return;
      }
      const { zoom: cur, pan: p } = view.current;
      if (cur > 1) applyView(cur, p.x - e.deltaX, p.y - e.deltaY);
    };
    let gestureStart = 1;
    type GestureEvt = Event & { scale: number; clientX: number; clientY: number };
    const onGestureStart = (e: Event) => { e.preventDefault(); gestureStart = view.current.zoom; };
    const onGestureChange = (e: Event) => {
      e.preventDefault();
      const g = e as GestureEvt;
      setScale(gestureStart * g.scale, { x: g.clientX, y: g.clientY });
    };
    stage.addEventListener("wheel", onWheel, { passive: false });
    stage.addEventListener("gesturestart", onGestureStart);
    stage.addEventListener("gesturechange", onGestureChange);
    stage.addEventListener("gestureend", onGestureStart);
    return () => {
      stage.removeEventListener("wheel", onWheel);
      stage.removeEventListener("gesturestart", onGestureStart);
      stage.removeEventListener("gesturechange", onGestureChange);
      stage.removeEventListener("gestureend", onGestureStart);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [enabled]);

  /** Spread onto the image: drag-to-pan once zoomed. */
  const dragHandlers = {
    onPointerDown: (e: ReactPointerEvent<HTMLElement>) => {
      if (view.current.zoom === 1) return;
      drag.current = { x: e.clientX, y: e.clientY, panX: view.current.pan.x, panY: view.current.pan.y };
      e.currentTarget.setPointerCapture(e.pointerId);
    },
    onPointerMove: (e: ReactPointerEvent<HTMLElement>) => {
      if (!drag.current) return;
      applyView(view.current.zoom, drag.current.panX + e.clientX - drag.current.x, drag.current.panY + e.clientY - drag.current.y);
    },
    onPointerUp: () => { drag.current = null; },
    onPointerCancel: () => { drag.current = null; },
  };

  return {
    zoom,
    pan,
    setScale,
    dragHandlers,
    transform: `translate(${pan.x}px, ${pan.y}px) scale(${zoom})`,
  };
}
