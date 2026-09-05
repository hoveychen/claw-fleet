import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { useTranslation } from "react-i18next";
import { imageFileName, sessionImageUrl } from "../sessionImages";
import styles from "./SessionImages.module.css";

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
  const [zoomed, setZoomed] = useState<string | null>(null);

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

  useEffect(() => {
    if (!zoomed) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") setZoomed(null);
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [zoomed]);

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
            <button
              key={img.path}
              className={styles.thumb}
              title={img.path}
              onClick={() => setZoomed(url)}
            >
              <img src={url} alt={name} loading="lazy" />
            </button>
          );
        })}
      </div>
      {zoomed && (
        <div className={styles.overlay} onClick={() => setZoomed(null)}>
          <img src={zoomed} alt="" />
        </div>
      )}
    </div>
  );
}
