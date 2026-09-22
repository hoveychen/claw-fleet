import { useTranslation } from "react-i18next";
import { Paperclip } from "lucide-react";
import { attachmentName } from "../../userAttachments";
import { useAttachmentThumb } from "../../attachmentThumb";
import { ImageThumbSrc } from "./ImageThumb";
import styles from "./AttachmentRow.module.css";

/**
 * The attachments on a user turn, shown wherever that turn is replayed: the
 * chat bubble and the decision-history record.
 *
 * An image renders as a thumbnail through the same `useAttachmentThumb` the
 * composer uses: a store path is served over `fleet-attachment://`, and a file
 * the user *picked* (which keeps its own path) is read via `read_external_file`.
 * The composer already showed the picked image before send, so replaying it as
 * a bare chip afterwards read as the image having been lost. Anything that is
 * not an image, or whose file is gone, stays a filename chip carrying the full
 * path in its tooltip.
 */
export function AttachmentRow({ paths }: { paths: string[] }) {
  const { t } = useTranslation();
  if (paths.length === 0) return null;

  return (
    <div className={styles.row}>
      {paths.map((path) => (
        <AttachmentItem key={path} path={path} />
      ))}
      <span className={styles.sr_only}>
        {t("detail.attachments", { defaultValue: "Attachments" })}
      </span>
    </div>
  );
}

function AttachmentItem({ path }: { path: string }) {
  const name = attachmentName(path);
  const src = useAttachmentThumb({ path, name });
  if (src) {
    return <ImageThumbSrc src={src} alt={name} />;
  }
  return (
    <span className={styles.chip} title={path}>
      <Paperclip size={12} className={styles.chip_icon} />
      <span className={styles.chip_name}>{name}</span>
    </span>
  );
}
