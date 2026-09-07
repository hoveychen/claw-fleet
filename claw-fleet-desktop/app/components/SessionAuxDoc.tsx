import { useTranslation } from "react-i18next";
import type { AuxDoc } from "../detailAux";
import { ExternalFilePreview } from "./FilesView";
import { WebTabPane } from "./WebTabPane";
import { WikiTabPane } from "./WikiTabPane";
import styles from "./SessionDetail.module.css";

/**
 * The strip of docs opened from agent prose, and the reader for the focused
 * one.
 *
 * A path, a `[[slug]]` or a url the agent named used to open as a tab in the
 * window's strip — which meant leaving the conversation to read the thing the
 * conversation was about. They open here instead, beside the sentence that
 * named them. The strip exists because a session names many: it keeps the last
 * few reachable without going back through the transcript to find the link.
 *
 * The three readers are the *same components* the 仓库 and 知识库 pages use, so
 * a file or doc looks identical wherever it is open.
 */
export function SessionAuxDocStrip({
  docs,
  activeId,
  onPick,
  onClose,
}: {
  docs: AuxDoc[];
  activeId: string | null;
  onPick: (id: string) => void;
  onClose: (id: string) => void;
}) {
  const { t } = useTranslation();
  if (docs.length === 0) return null;
  return (
    <div className={styles.aux_docs}>
      {docs.map((d) => (
        <span
          key={d.id}
          className={`${styles.aux_doc_chip} ${activeId === d.id ? styles.aux_doc_chip_active : ""}`}
        >
          <button
            type="button"
            className={styles.aux_doc_chip_label}
            onClick={() => onPick(d.id)}
            title={d.ref}
          >
            {d.label}
          </button>
          <button
            type="button"
            className={styles.aux_doc_chip_close}
            onClick={() => onClose(d.id)}
            aria-label={t("common.close", "关闭")}
          >
            ✕
          </button>
        </span>
      ))}
    </div>
  );
}

export function SessionAuxDoc({
  doc,
  onOpenWiki,
  onClose,
}: {
  doc: AuxDoc;
  onOpenWiki: (slug: string) => void;
  onClose: () => void;
}) {
  const { t } = useTranslation();
  switch (doc.kind) {
    case "file":
      return (
        <div className={styles.aux_doc_pane}>
          <ExternalFilePreview
            path={doc.ref}
            // Read-only, and — unlike the 仓库 page's use of this component —
            // usually a file that IS inside a workspace.
            label={t("tabs.file_readonly", "只读预览")}
            onClose={onClose}
          />
        </div>
      );
    case "wiki":
      return (
        <div className={styles.aux_doc_pane}>
          {/* A `[[slug]]` inside the doc opens the next one in this same
              panel, so following a chain of wiki refs never leaves the
              conversation. */}
          <WikiTabPane slug={doc.ref} onOpenSlug={onOpenWiki} />
        </div>
      );
    case "web":
      return (
        <div className={styles.aux_doc_pane}>
          <WebTabPane url={doc.ref} />
        </div>
      );
  }
}
