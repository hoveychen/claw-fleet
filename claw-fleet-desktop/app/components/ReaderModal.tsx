import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { BookUp2, Printer } from "lucide-react";
import { CopyButton } from "./CopyButton";
import { TextBlock } from "./blocks/TextBlock";
import { WikiPublishDialog } from "./WikiPublishDialog";
import type { WikiDoc } from "./WikiView";
import type { PathLinkContext } from "../markdown/pathLinks";
import { normalizeSlug } from "../wikiSlug";
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

/** `notes/<workspace>-<yyyy-mm-dd>`: one running note per workspace per day, so
 *  a second export the same day defaults to appending onto the first. */
function defaultSlug(workspaceRoot: string | undefined): string {
  const name = (workspaceRoot ?? "").split("/").filter(Boolean).pop() ?? "reader";
  const d = new Date();
  const day = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  // The workspace name can be anything on disk; fold it the way core will.
  return `notes/${normalizeSlug(name) || "reader"}-${day}`;
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
  const [publishing, setPublishing] = useState(false);
  /** Transient confirmation shown in the header after a successful publish. */
  const [published, setPublished] = useState<string | null>(null);
  const [printError, setPrintError] = useState(false);

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

  useEffect(() => {
    if (!published) return;
    const id = window.setTimeout(() => setPublished(null), 2600);
    return () => window.clearTimeout(id);
  }, [published]);

  // Printing has to go through the host: `window.print()` is a no-op inside
  // WKWebView, so a frontend-only call would do nothing at all on macOS. What
  // reaches the page is decided by the print stylesheet, which hides everything
  // except this reader's body.
  const print = async () => {
    setPrintError(false);
    try {
      await invoke("print_webview");
    } catch {
      setPrintError(true);
    }
  };

  const onPublished = (doc: WikiDoc, appended: boolean) => {
    setPublishing(false);
    setPublished(
      appended
        ? t("wiki.publish_appended_toast", { slug: doc.slug })
        : t("wiki.publish_done_toast", { slug: doc.slug }),
    );
  };

  return createPortal(
    <div className={styles.overlay} onClick={onClose} data-reader-overlay>
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
            {published && <span className={styles.toast}>{published}</span>}
            <button
              type="button"
              className={styles.action}
              onClick={() => setPublishing(true)}
              title={t("wiki.publish_action")}
              aria-label={t("wiki.publish_action")}
            >
              <BookUp2 size={14} strokeWidth={1.75} />
            </button>
            <button
              type="button"
              className={`${styles.action} ${printError ? styles.action_failed : ""}`}
              onClick={print}
              title={printError ? t("detail.reader_print_failed") : t("detail.reader_print")}
              aria-label={t("detail.reader_print")}
            >
              <Printer size={14} strokeWidth={1.75} />
            </button>
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
        <div className={styles.body} data-reader-body>
          <TextBlock text={text} paths={paths} />
        </div>
      </div>
      {publishing && (
        <WikiPublishDialog
          text={text}
          workspacePath={paths?.workspaceRoot ?? ""}
          defaultSlug={defaultSlug(paths?.workspaceRoot)}
          onClose={() => setPublishing(false)}
          onPublished={onPublished}
        />
      )}
    </div>,
    document.body,
  );
}
