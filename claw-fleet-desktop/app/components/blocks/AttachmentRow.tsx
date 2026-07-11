import { useTranslation } from "react-i18next";
import { Paperclip } from "lucide-react";
import {
  attachmentName,
  isRenderableImage,
  userAttachmentUrl,
} from "../../userAttachments";
import { ImageThumbSrc } from "./ImageThumb";
import styles from "./AttachmentRow.module.css";

/**
 * The attachments on a user turn, shown wherever that turn is replayed: the
 * chat bubble and the decision-history record.
 *
 * An image in the persistent store renders as a thumbnail — the point of the
 * store is that its bytes are still there weeks later, on whichever host the
 * agent runs on. Everything else renders as a filename chip carrying the full
 * path in its tooltip: a file the user *picked* keeps its own path, and we have
 * no license to read arbitrary paths off the agent's disk just to preview them.
 */
export function AttachmentRow({ paths }: { paths: string[] }) {
  const { t } = useTranslation();
  if (paths.length === 0) return null;

  return (
    <div className={styles.row}>
      {paths.map((path) => {
        const name = attachmentName(path);
        const url = isRenderableImage(name) ? userAttachmentUrl(path) : null;
        if (url) {
          return <ImageThumbSrc key={path} src={url} alt={name} />;
        }
        return (
          <span key={path} className={styles.chip} title={path}>
            <Paperclip size={12} className={styles.chip_icon} />
            <span className={styles.chip_name}>{name}</span>
          </span>
        );
      })}
      <span className={styles.sr_only}>
        {t("detail.attachments", { defaultValue: "Attachments" })}
      </span>
    </div>
  );
}
