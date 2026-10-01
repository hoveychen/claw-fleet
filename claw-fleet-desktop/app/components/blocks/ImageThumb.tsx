import { useState } from "react";
import { useTranslation } from "react-i18next";
import { ImageOff, Scissors } from "lucide-react";
import type { ImageBlock } from "../../types";
import { imageDataUrl, isTrimmedImageData } from "../../imageData";
import { ImageLightbox } from "../ImageLightbox";
import styles from "./ImageThumb.module.css";
import { Presence } from "../Presence";
import { Skeleton } from "../loading";
import { useDelayedFlag } from "../../hooks/useDelayedFlag";

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
  // Keyed by src so a thumb handed a new image goes back to its loading box.
  const [loadedSrc, setLoadedSrc] = useState<string | null>(null);
  const loaded = loadedSrc === src;
  // Until the image decodes it has no size, so the thumb would sit as an empty
  // zero-height button. Reserve a thumb-sized box instead — but only after the
  // show-delay, so an inline data URL that decodes at once never flashes it.
  const showSkeleton = useDelayedFlag(!loaded);
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
        onClick={() => {
          setLoadedSrc(null);
          setBroken(false);
        }}
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
        aria-busy={!loaded || undefined}
      >
        {!loaded && showSkeleton && (
          <Skeleton inline width={160} height={110} radius={0} />
        )}
        <img
          src={src}
          alt={alt}
          className={loaded ? styles.thumb : styles.thumb_pending}
          onLoad={() => setLoadedSrc(src)}
          onError={() => setBroken(true)}
        />
      </button>
      <Presence when={Boolean(zoomed)}>{zoomed && <ImageLightbox src={src} alt={alt} onClose={() => setZoomed(false)} />}</Presence>
    </>
  );
}
