import { useEffect } from "react";
import { createPortal } from "react-dom";
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

  // Rendered through a portal to document.body so the `position: fixed` overlay
  // covers the whole viewport. Rendered inline it would be trapped by any
  // ancestor that establishes a containing block for fixed positioning — e.g.
  // the DecisionPanel card (`.panel` has `transform: translateX(-50%)`), which
  // confined the overlay to the ~460px card box instead of the full window.
  return createPortal(
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
    </div>,
    document.body,
  );
}
