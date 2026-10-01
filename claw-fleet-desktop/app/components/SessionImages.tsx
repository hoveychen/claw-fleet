import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { imageFileName, sessionImageUrl } from "../sessionImages";
import { ImageLightbox } from "./ImageLightbox";
import styles from "./SessionImages.module.css";
import { Presence } from "./Presence";
import { Skeleton } from "./loading";

interface GeneratedImage {
  path: string;
  bytes: number;
}

interface Props {
  /** Fleet session id — for a Codex session this *is* the Codex thread id. */
  sessionId: string;
}

/**
 * Thumbnails of the images a Codex session generated with the built-in
 * `image_gen` tool.
 *
 * Renders nothing at all when there are none, which is the common case (every
 * Claude session, and every Codex session that never made a picture) — same
 * self-hiding contract as `SkillHistory` in `inline` mode, so the detail modal
 * gains no empty chrome.
 */
export function SessionImages({ sessionId }: Props) {
  const { t } = useTranslation();
  const [images, setImages] = useState<GeneratedImage[]>([]);
  const [zoomed, setZoomed] = useState<{ src: string; name: string } | null>(null);

  useEffect(() => {
    // Guarded against a late reply for a session the user already navigated
    // away from — otherwise the strip briefly shows the previous session's
    // pictures under the new session's header.
    let current = true;
    invoke<GeneratedImage[]>("list_session_images", { sessionId })
      .then((list) => {
        if (current) setImages(list);
      })
      .catch(() => {
        if (current) setImages([]);
      });
    return () => {
      current = false;
    };
  }, [sessionId]);

  if (images.length === 0) return null;

  return (
    <div className={styles.root}>
      <div className={styles.title}>
        {t("generated_images", { count: images.length })}
      </div>
      <div className={styles.strip}>
        {images.map((img) => {
          const name = imageFileName(img.path);
          const url = sessionImageUrl(sessionId, name);
          return (
            <Thumb
              key={img.path}
              url={url}
              name={name}
              title={img.path}
              onOpen={() => setZoomed({ src: url, name })}
            />
          );
        })}
      </div>
      <Presence when={Boolean(zoomed)}>{zoomed && <ImageLightbox src={zoomed.src} alt={zoomed.name} onClose={() => setZoomed(null)} />}</Presence>
    </div>
  );
}

/** One thumbnail. Holds a square shimmer until the image decodes, so the strip
 *  does not show empty bordered slivers that then widen one by one. */
function Thumb({
  url,
  name,
  title,
  onOpen,
}: {
  url: string;
  name: string;
  title: string;
  onOpen: () => void;
}) {
  const [ready, setReady] = useState(false);
  return (
    <button className={styles.thumb} title={title} onClick={onOpen}>
      {!ready && <Skeleton className={styles.thumb_skeleton} width="100%" height="100%" radius={0} />}
      <img
        src={url}
        alt={name}
        loading="lazy"
        className={ready ? undefined : styles.img_pending}
        // A broken image clears the shimmer too: the browser's own broken-image
        // glyph is the error state, and a shimmer must never outlive the request.
        onLoad={() => setReady(true)}
        onError={() => setReady(true)}
      />
    </button>
  );
}
