import { useRef, type ImgHTMLAttributes } from "react";
import { useTranslation } from "react-i18next";
import { Minus, Plus, RotateCcw } from "lucide-react";

import { MAX_ZOOM, MIN_ZOOM, useZoomPan } from "../hooks/useZoomPan";
import { useAuxMaximized } from "./AuxDocBar";
import styles from "./ZoomableImage.module.css";

/**
 * An image a reader card shows, which gains zoom and pan once its card is
 * maximized: pinch / ⌘+wheel to zoom around the pointer, drag or two-finger
 * scroll to pan, double-click to toggle 2× / fit, and a small −/%/+/reset pill.
 *
 * In the rail it is a plain `<img>`: zooming there would capture the wheel and
 * stop the rail from scrolling, and a 320px card has no room to zoom into.
 */
export function ZoomableImage({
  className,
  style,
  ...img
}: ImgHTMLAttributes<HTMLImageElement>) {
  const { t } = useTranslation();
  const maximized = useAuxMaximized();
  const stageRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const { zoom, setScale, dragHandlers, transform } = useZoomPan(stageRef, imgRef, maximized);

  if (!maximized) return <img className={className} style={style} {...img} />;

  return (
    <div ref={stageRef} className={styles.stage}>
      <img
        ref={imgRef}
        className={`${className ?? ""} ${styles.image}`}
        style={{ ...style, transform, cursor: zoom > 1 ? "grab" : "zoom-in" }}
        draggable={false}
        onDoubleClick={(e) =>
          setScale(zoom > 1 ? 1 : 2, { x: e.clientX, y: e.clientY })
        }
        {...dragHandlers}
        {...img}
      />
      <div className={styles.controls}>
        <button
          type="button"
          onClick={() => setScale(zoom - 0.5)}
          disabled={zoom <= MIN_ZOOM}
          title={t("composer.lightbox_zoom_out", "缩小")}
          aria-label={t("composer.lightbox_zoom_out", "缩小")}
        >
          <Minus size={13} />
        </button>
        <span className={styles.zoom}>{Math.round(zoom * 100)}%</span>
        <button
          type="button"
          onClick={() => setScale(zoom + 0.5)}
          disabled={zoom >= MAX_ZOOM}
          title={t("composer.lightbox_zoom_in", "放大")}
          aria-label={t("composer.lightbox_zoom_in", "放大")}
        >
          <Plus size={13} />
        </button>
        <button
          type="button"
          onClick={() => setScale(1)}
          disabled={zoom === 1}
          title={t("composer.lightbox_reset", "适合窗口")}
          aria-label={t("composer.lightbox_reset", "适合窗口")}
        >
          <RotateCcw size={12} />
        </button>
      </div>
    </div>
  );
}
