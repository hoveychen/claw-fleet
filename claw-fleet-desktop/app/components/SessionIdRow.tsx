import { useTranslation } from "react-i18next";
import { FileJson2 } from "lucide-react";
import { CopyButton } from "./CopyButton";
import styles from "./SessionIdRow.module.css";

/**
 * Surfaces the session id and its transcript path so either can be pasted to
 * an agent.
 *
 * The id is the transcript filename stem, so an agent handed just the id can
 * glob it out of `~/.claude/projects/<proj>/<id>.jsonl`; the path button is a
 * shortcut that saves it the glob.
 */
export function SessionIdRow({
  id,
  jsonlPath,
  className,
}: {
  id: string;
  jsonlPath: string;
  className?: string;
}) {
  const { t } = useTranslation();

  return (
    <div className={`${styles.row} ${className ?? ""}`}>
      <span className={styles.label}>{t("detail.session_id")}</span>
      <code className={styles.value} title={id}>
        {id}
      </code>
      <CopyButton
        text={id}
        label={t("detail.copy_session_id")}
        className={styles.btn}
      />
      <CopyButton
        text={jsonlPath}
        label={t("detail.copy_transcript_path")}
        icon={<FileJson2 size={12} />}
        className={styles.btn}
      />
    </div>
  );
}
