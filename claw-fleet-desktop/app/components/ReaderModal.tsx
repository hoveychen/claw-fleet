import { useEffect } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { CopyButton } from "./CopyButton";
import { TextBlock } from "./blocks/TextBlock";
import type { PathLinkContext } from "../markdown/pathLinks";
import styles from "./ReaderModal.module.css";

interface Props {
  /** Markdown source of the message — `messageToText`'s output, the same
   *  string the copy control puts on the clipboard. */
  text: string;
  /** Shown in the header so a reader can tell whose turn they opened. */
  title?: string;
  paths?: PathLinkContext;
  onClose: () => void;
}

/**
 * Full-screen reading view for one message.
 *
 * The transcript column is optimised for scanning a conversation: 13px prose in
 * a column as wide as the window happens to be. That is the wrong shape for
 * actually *reading* a long answer, so this lifts the same markdown onto a
 * narrow, larger-type page with generous leading.
 *
 * Rendered through a portal to document.body for the same reason ImageLightbox
 * is: a `position: fixed` overlay nested inside a transformed ancestor would be
 * clipped to that ancestor's box instead of covering the window.
 */
export function ReaderModal({ text, title, paths, onClose }: Props) {
  const { t } = useTranslation();

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        // Reading is a modal detour: swallow the key so the same Escape does
        // not also close whatever opened underneath (detail tab, inspect modal).
        e.stopPropagation();
        onClose();
      }
    };
    // Capture phase, so ancestors listening on window still lose the race.
    window.addEventListener("keydown", handler, true);
    return () => window.removeEventListener("keydown", handler, true);
  }, [onClose]);

  return createPortal(
    <div className={styles.overlay} onClick={onClose}>
      <div
        className={styles.paper}
        onClick={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={t("detail.reader_title")}
      >
        <div className={styles.header}>
          <span className={styles.title}>{title ?? t("detail.reader_title")}</span>
          <div className={styles.actions}>
            <CopyButton text={text} />
            <button
              type="button"
              className={styles.close}
              onClick={onClose}
              title={t("detail.reader_close")}
              aria-label={t("detail.reader_close")}
            >
              ×
            </button>
          </div>
        </div>
        <div className={styles.body}>
          <TextBlock text={text} paths={paths} />
        </div>
      </div>
    </div>,
    document.body,
  );
}
