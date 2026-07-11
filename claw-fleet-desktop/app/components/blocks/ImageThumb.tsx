import { useState } from "react";
import type { ImageBlock } from "../../types";
import { imageDataUrl } from "../../imageData";
import { ImageLightbox } from "../ImageLightbox";
import styles from "./ImageThumb.module.css";

/** A capped thumbnail that opens the full image in a lightbox. */
export function ImageThumb({ block, alt }: { block: ImageBlock; alt: string }) {
  const src = imageDataUrl(block);
  if (!src) return null;
  return <ImageThumbSrc src={src} alt={alt} />;
}

/**
 * [`ImageThumb`] for images that already have a URL instead of inline base64 —
 * attachments served out of the store over `fleet-attachment://`.
 */
export function ImageThumbSrc({ src, alt }: { src: string; alt: string }) {
  const [zoomed, setZoomed] = useState(false);
  const [broken, setBroken] = useState(false);
  // The file can be gone (an attachment from before the store existed, or a
  // store pruned by hand). Showing nothing beats a broken-image glyph; the
  // caller still renders the filename chip next to it.
  if (broken) return null;
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
