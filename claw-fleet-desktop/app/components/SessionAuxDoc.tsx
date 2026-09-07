import { useTranslation } from "react-i18next";
import type { AuxDoc } from "../detailAux";
import { ExternalFilePreview } from "./FilesView";
import { WebTabPane } from "./WebTabPane";
import { WikiTabPane } from "./WikiTabPane";
import styles from "./SessionDetail.module.css";

/**
 * The reader for a doc opened from agent prose.
 *
 * A path, a `[[slug]]` or a url the agent named used to open as a tab in the
 * *window's* strip — which meant leaving the conversation to read the thing the
 * conversation was about. It opens as a tab in the auxiliary column instead,
 * beside the sentence that named it.
 *
 * The three readers are the *same components* the 仓库 and 知识库 pages use, so
 * a file or doc looks identical wherever it is open.
 */
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
