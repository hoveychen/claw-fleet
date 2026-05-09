import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import styles from "./ImageLightbox.module.css";

interface Props {
  src: string;
  alt?: string;
  onClose: () => void;
}

export function ImageLightbox({ src, alt, onClose }: Props) {
  const { t } = useTranslation();

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  return (
    <div className={styles.overlay} onClick={onClose}>
      <button
        type="button"
        className={styles.close}
        onClick={onClose}
        title={t("composer.lightbox_close", "Close")}
        aria-label={t("composer.lightbox_close", "Close")}
      >
        ×
      </button>
      <img
        src={src}
        alt={alt ?? ""}
        className={styles.image}
        onClick={(e) => e.stopPropagation()}
        draggable={false}
      />
    </div>
  );
}
