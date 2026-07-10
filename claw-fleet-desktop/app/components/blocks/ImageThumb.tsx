import { useState } from "react";
import type { ImageBlock } from "../../types";
import { imageDataUrl } from "../../imageData";
import { ImageLightbox } from "../ImageLightbox";
import styles from "./ImageThumb.module.css";

/** A capped thumbnail that opens the full image in a lightbox. */
export function ImageThumb({ block, alt }: { block: ImageBlock; alt: string }) {
  const [zoomed, setZoomed] = useState(false);
  const src = imageDataUrl(block);
  if (!src) return null;
  return (
    <>
      <button
        type="button"
        className={styles.thumb_btn}
        onClick={() => setZoomed(true)}
        title={alt}
      >
        <img src={src} alt={alt} className={styles.thumb} />
      </button>
      {zoomed && <ImageLightbox src={src} alt={alt} onClose={() => setZoomed(false)} />}
    </>
  );
}
