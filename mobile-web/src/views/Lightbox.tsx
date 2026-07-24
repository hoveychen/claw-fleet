import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";
import { X } from "lucide-react";
import { HistoryLayer } from "../useNavStack";
import { t } from "../i18n";
import styles from "./Lightbox.module.css";

// ── public API ────────────────────────────────────────────────────────────────

interface LightboxApi {
  /** Open the full-screen viewer for one image (data URI or URL). */
  open: (src: string, alt?: string) => void;
}

const NOOP: LightboxApi = { open: () => {} };
const LightboxContext = createContext<LightboxApi>(NOOP);

/** Any `<img>` render site (chat thumbnails, tool results, decision galleries)
 *  calls `useLightbox().open(src)` to enlarge. Falls back to a no-op when no
 *  provider is mounted (e.g. an isolated unit render) so callers never crash. */
export function useLightbox(): LightboxApi {
  return useContext(LightboxContext);
}

export function LightboxProvider({ children }: { children: ReactNode }) {
  const [img, setImg] = useState<{ src: string; alt: string } | null>(null);
  const open = useCallback((src: string, alt = "") => setImg({ src, alt }), []);
  const close = useCallback(() => setImg(null), []);
  const api = useMemo(() => ({ open }), [open]);
  return (
    <LightboxContext.Provider value={api}>
      {children}
      {img && <LightboxOverlay src={img.src} alt={img.alt} onClose={close} />}
    </LightboxContext.Provider>
  );
}

// ── overlay + gestures ──────────────────────────────────────────────────────

const MIN_SCALE = 1;
const MAX_SCALE = 6;
const DOUBLE_TAP_SCALE = 2.5;
const DOUBLE_TAP_MS = 300;
// Movement past this (px) during a one-finger touch means "drag", not "tap".
const TAP_SLOP = 10;

type Gesture =
  | { mode: "idle" }
  | {
      mode: "pan";
      startX: number;
      startY: number;
      startTx: number;
      startTy: number;
      moved: boolean;
    }
  | {
      mode: "pinch";
      startDist: number;
      startScale: number;
      startTx: number;
      startTy: number;
      // Pinch focal point, in viewport-center-relative coords, captured at start.
      focalX: number;
      focalY: number;
    };

function dist(a: Touch, b: Touch): number {
  return Math.hypot(a.clientX - b.clientX, a.clientY - b.clientY);
}

function clamp(v: number, lo: number, hi: number): number {
  return Math.min(hi, Math.max(lo, v));
}

function LightboxOverlay({
  src,
  alt,
  onClose,
}: {
  src: string;
  alt: string;
  onClose: () => void;
}) {
  const rootRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  // Committed transform (between gestures). Live gesture values are written
  // straight to the element for smoothness, then committed here on touchend.
  const tf = useRef({ scale: 1, tx: 0, ty: 0 });
  const gesture = useRef<Gesture>({ mode: "idle" });
  const lastTapAt = useRef(0);
  const singleTapTimer = useRef<number | undefined>(undefined);

  const apply = useCallback((animate: boolean) => {
    const el = imgRef.current;
    if (!el) return;
    const { scale, tx, ty } = tf.current;
    el.style.transition = animate ? "transform 0.2s ease" : "none";
    el.style.transform = `translate(${tx}px, ${ty}px) scale(${scale})`;
  }, []);

  // Keep the image on screen: don't let it be panned entirely out of view.
  const clampPan = useCallback(() => {
    const el = imgRef.current;
    if (!el) return;
    const { scale } = tf.current;
    // offsetWidth/Height are the untransformed layout size — the transform
    // (scale) doesn't affect them, so scaledW is the true rendered width.
    const maxTx = Math.max(0, (el.offsetWidth * scale - window.innerWidth) / 2);
    const maxTy = Math.max(0, (el.offsetHeight * scale - window.innerHeight) / 2);
    tf.current.tx = clamp(tf.current.tx, -maxTx, maxTx);
    tf.current.ty = clamp(tf.current.ty, -maxTy, maxTy);
  }, []);

  const zoomTo = useCallback(
    (scale: number, focalX = 0, focalY = 0) => {
      const prev = tf.current.scale;
      const next = clamp(scale, MIN_SCALE, MAX_SCALE);
      if (next <= MIN_SCALE + 0.001) {
        tf.current = { scale: MIN_SCALE, tx: 0, ty: 0 };
      } else {
        // Keep the point under `focal` fixed while scaling around center.
        tf.current.tx = focalX - (next / prev) * (focalX - tf.current.tx);
        tf.current.ty = focalY - (next / prev) * (focalY - tf.current.ty);
        tf.current.scale = next;
        clampPan();
      }
      apply(true);
    },
    [apply, clampPan],
  );

  // Native listeners (not React's synthetic, which are passive by default) so
  // preventDefault actually suppresses page scroll / pull-to-refresh while we
  // drive zoom & pan ourselves. lockZoom already blocks native pinch globally;
  // our transform-based zoom is independent of it.
  useEffect(() => {
    const root = rootRef.current;
    if (!root) return;
    const cx = () => window.innerWidth / 2;
    const cy = () => window.innerHeight / 2;

    const onStart = (e: TouchEvent) => {
      if (e.touches.length >= 2) {
        const [a, b] = [e.touches[0], e.touches[1]];
        gesture.current = {
          mode: "pinch",
          startDist: dist(a, b),
          startScale: tf.current.scale,
          startTx: tf.current.tx,
          startTy: tf.current.ty,
          focalX: (a.clientX + b.clientX) / 2 - cx(),
          focalY: (a.clientY + b.clientY) / 2 - cy(),
        };
      } else if (e.touches.length === 1) {
        const tch = e.touches[0];
        gesture.current = {
          mode: "pan",
          startX: tch.clientX,
          startY: tch.clientY,
          startTx: tf.current.tx,
          startTy: tf.current.ty,
          moved: false,
        };
      }
    };

    const onMove = (e: TouchEvent) => {
      const g = gesture.current;
      if (g.mode === "pinch" && e.touches.length >= 2) {
        e.preventDefault();
        const ratio = dist(e.touches[0], e.touches[1]) / g.startDist;
        const next = clamp(g.startScale * ratio, MIN_SCALE, MAX_SCALE);
        tf.current.scale = next;
        tf.current.tx = g.focalX - (next / g.startScale) * (g.focalX - g.startTx);
        tf.current.ty = g.focalY - (next / g.startScale) * (g.focalY - g.startTy);
        clampPan();
        apply(false);
      } else if (g.mode === "pan" && e.touches.length === 1) {
        const tch = e.touches[0];
        const dx = tch.clientX - g.startX;
        const dy = tch.clientY - g.startY;
        if (Math.hypot(dx, dy) > TAP_SLOP) g.moved = true;
        // Only pan when zoomed in — at fit scale the image shouldn't slide.
        if (tf.current.scale > MIN_SCALE + 0.001) {
          e.preventDefault();
          tf.current.tx = g.startTx + dx;
          tf.current.ty = g.startTy + dy;
          clampPan();
          apply(false);
        }
      }
    };

    const onEnd = (e: TouchEvent) => {
      const g = gesture.current;
      // Fingers lifted from a pinch — settle. A pinch that ended below fit
      // snaps back to 1 (handled by zoomTo's MIN clamp).
      if (g.mode === "pinch") {
        if (tf.current.scale <= MIN_SCALE + 0.001) zoomTo(MIN_SCALE);
        gesture.current = { mode: "idle" };
        return;
      }
      if (g.mode === "pan") {
        gesture.current = { mode: "idle" };
        if (g.moved) return; // a drag, not a tap
        // Tap. Distinguish single (→ close at fit) from double (→ zoom toggle).
        const now = e.timeStamp;
        const tch = e.changedTouches[0];
        if (now - lastTapAt.current <= DOUBLE_TAP_MS) {
          // Double tap: cancel the pending single-tap close, toggle zoom.
          if (singleTapTimer.current !== undefined) {
            window.clearTimeout(singleTapTimer.current);
            singleTapTimer.current = undefined;
          }
          lastTapAt.current = 0;
          if (tf.current.scale > MIN_SCALE + 0.001) {
            zoomTo(MIN_SCALE);
          } else {
            zoomTo(DOUBLE_TAP_SCALE, tch.clientX - cx(), tch.clientY - cy());
          }
        } else {
          lastTapAt.current = now;
          // At fit scale a lone tap dismisses; wait out the double-tap window
          // so a real double tap zooms instead of closing.
          if (tf.current.scale <= MIN_SCALE + 0.001) {
            singleTapTimer.current = window.setTimeout(() => {
              singleTapTimer.current = undefined;
              onClose();
            }, DOUBLE_TAP_MS);
          }
        }
      }
    };

    root.addEventListener("touchstart", onStart, { passive: false });
    root.addEventListener("touchmove", onMove, { passive: false });
    root.addEventListener("touchend", onEnd, { passive: false });
    root.addEventListener("touchcancel", onEnd, { passive: false });
    return () => {
      root.removeEventListener("touchstart", onStart);
      root.removeEventListener("touchmove", onMove);
      root.removeEventListener("touchend", onEnd);
      root.removeEventListener("touchcancel", onEnd);
      if (singleTapTimer.current !== undefined) window.clearTimeout(singleTapTimer.current);
    };
  }, [apply, clampPan, zoomTo, onClose]);

  return (
    <div
      ref={rootRef}
      className={styles.overlay}
      role="dialog"
      aria-modal="true"
      aria-label={t("图片查看")}
    >
      {/* 硬件/手势返回先关灯箱，和其它浮层一致。 */}
      <HistoryLayer onBack={onClose} />
      <img ref={imgRef} className={styles.image} src={src} alt={alt} draggable={false} />
      <button
        type="button"
        className={styles.close}
        aria-label={t("关闭")}
        // Pointer, not touch — the overlay's touch handlers own the canvas; the
        // button is a plain click target above them.
        onClick={onClose}
      >
        <X size={22} />
      </button>
      <div className={styles.hint}>{t("双击放大 · 捏合缩放 · 单击关闭")}</div>
    </div>
  );
}
