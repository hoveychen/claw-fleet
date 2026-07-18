import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ImageOff, Scissors } from "lucide-react";
import type { ImageBlock } from "../../types";
import { imageDataUrl, isTrimmedImageData } from "../../imageData";
import { ImageLightbox } from "../ImageLightbox";
import styles from "./ImageThumb.module.css";

/** A capped thumbnail that opens the full image in a lightbox. */
export function ImageThumb({ block, alt }: { block: ImageBlock; alt: string }) {
  const { t } = useTranslation();
  // Transport-trimmed base64 (marker inside the data) can never decode —
  // rendering it as an <img> guarantees a broken load. Name the state instead;
  // the owning card recovers the real payload via `get_tool_result_full`.
  if (isTrimmedImageData(block)) {
    return (
      <div className={styles.fallback} data-testid="image-thumb-truncated">
        <Scissors size={14} aria-hidden />
        <span>{t("detail.image_truncated")}</span>
      </div>
    );
  }
  const src = imageDataUrl(block);
  if (!src) return null;
  return <ImageThumbSrc src={src} alt={alt} />;
}

/**
 * [`ImageThumb`] for images that already have a URL instead of inline base64 —
 * attachments served out of the store over `fleet-attachment://`.
 */
export function ImageThumbSrc({ src, alt }: { src: string; alt: string }) {
  const { t } = useTranslation();
  const [zoomed, setZoomed] = useState(false);
  const [broken, setBroken] = useState(false);
  // A failed load must stay visible. This component used to render nothing
  // here, which collapsed every upstream fault — corrupt payload, pruned
  // attachment store, dropped refetch — into the same silent empty box that no
  // one could diagnose from the UI. Clicking retries (clearing `broken`
  // re-mounts the <img> so the engine re-attempts the load).
  if (broken) {
    return (
      <button
        type="button"
        className={styles.fallback_btn}
        data-testid="image-thumb-broken"
        onClick={() => setBroken(false)}
        title={alt}
      >
        <ImageOff size={14} aria-hidden />
        <span>{t("detail.image_broken")}</span>
      </button>
    );
  }
  return (
    <>
      <button
        type="button"
        className={styles.thumb_btn}
        onClick={() => setZoomed(true)}
        title={alt}
      >
        <img
          src={src}
          alt={alt}
          className={styles.thumb}
          onError={() => setBroken(true)}
        />
      </button>
      {zoomed && <ImageLightbox src={src} alt={alt} onClose={() => setZoomed(false)} />}
    </>
  );
}
