// The phone's 资料 upload: photograph receipts (or pick files), choose the
// project they belong to, and send them into one ingest batch — the host's AI
// reads, titles and files each one. Taking several shots before sending makes
// them one batch, so the summary shows them together.

import { useEffect, useMemo, useRef, useState } from "react";
import { Camera, ChevronDown, FileText, Paperclip, X } from "lucide-react";
import { t } from "../i18n";
import type { FleetTransport } from "../transport";
import { formatBytes } from "../artifacts";
import {
  MAX_LIBRARY_UPLOAD_BYTES,
  UploadSkippedAll,
  uploadToLibrary,
  type UploadOutcome,
  type UploadProgress,
} from "../libraryApi";
import { Spinner } from "./loading";
import treeStyles from "./LibraryTree.module.css";
import styles from "./LibraryUpload.module.css";

interface Picked {
  key: number;
  file: File;
  /** `blob:` preview for an image. */
  preview?: string;
}

let pickSeq = 0;

export function UploadSheet({
  client,
  projects,
  project,
  onProject,
  onClose,
  onUploaded,
}: {
  client: FleetTransport | null;
  projects: Array<{ path: string; name: string }>;
  /** The project to upload into ("" = not chosen yet). */
  project: string;
  onProject: (path: string) => void;
  onClose: () => void;
  onUploaded: (outcome: UploadOutcome) => void;
}) {
  const [picked, setPicked] = useState<Picked[]>([]);
  const [progress, setProgress] = useState<UploadProgress | null>(null);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cameraRef = useRef<HTMLInputElement>(null);
  const filesRef = useRef<HTMLInputElement>(null);

  useEffect(
    () => () => {
      for (const p of picked) if (p.preview) URL.revokeObjectURL(p.preview);
    },
    // Revoke on unmount only; removing one revokes it there.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [],
  );

  const add = (list: FileList | null) => {
    if (!list || list.length === 0) return;
    const next = Array.from(list).map((file) => ({
      key: ++pickSeq,
      file,
      preview: file.type.startsWith("image/") ? URL.createObjectURL(file) : undefined,
    }));
    setPicked((prev) => [...prev, ...next]);
    setError(null);
  };
  const remove = (key: number) =>
    setPicked((prev) => {
      const gone = prev.find((p) => p.key === key);
      if (gone?.preview) URL.revokeObjectURL(gone.preview);
      return prev.filter((p) => p.key !== key);
    });

  const tooBig = picked.filter((p) => p.file.size > MAX_LIBRARY_UPLOAD_BYTES).length;
  const totalBytes = useMemo(() => picked.reduce((s, p) => s + p.file.size, 0), [picked]);
  const canSend = !!client && !!project && picked.length > tooBig && !sending;

  const send = async () => {
    if (!client || !canSend) return;
    setSending(true);
    setError(null);
    setProgress({ file: 0, files: picked.length, sentBytes: 0, totalBytes });
    try {
      const outcome = await uploadToLibrary(
        client,
        project,
        picked.map((p) => p.file),
        setProgress,
      );
      onUploaded(outcome);
    } catch (e) {
      setError(
        e instanceof UploadSkippedAll
          ? t("一个文件都没传上：{0}", e.message)
          : t("上传失败：{0}", e instanceof Error ? e.message : String(e)),
      );
      setSending(false);
    }
  };

  const pct =
    progress && progress.totalBytes > 0
      ? Math.round((progress.sentBytes / progress.totalBytes) * 100)
      : 0;

  return (
    <div className={treeStyles.backdrop} onClick={sending ? undefined : onClose}>
      <div
        className={`${treeStyles.sheet} ${styles.sheet}`}
        onClick={(e) => e.stopPropagation()}
        data-upload-sheet
      >
        <div className={treeStyles.sheetHead}>
          <span className={treeStyles.sheetTitle}>{t("上传到资料")}</span>
          <button
            type="button"
            className={treeStyles.close}
            onClick={onClose}
            disabled={sending}
            aria-label={t("关闭")}
          >
            <X size={18} />
          </button>
        </div>

        <label className={styles.projectWrap} data-empty={!project}>
          <span className={styles.projectLabel}>{t("项目")}</span>
          <select
            className={styles.project}
            value={project}
            disabled={sending}
            onChange={(e) => onProject(e.target.value)}
          >
            <option value="">{t("选择项目…")}</option>
            {projects.map((p) => (
              <option key={p.path} value={p.path}>
                {p.name}
              </option>
            ))}
          </select>
          <ChevronDown size={14} aria-hidden="true" />
        </label>

        <div className={styles.pickers}>
          <button
            type="button"
            className={styles.pick}
            disabled={sending}
            onClick={() => cameraRef.current?.click()}
          >
            <Camera size={22} strokeWidth={1.6} />
            {t("拍照")}
          </button>
          <button
            type="button"
            className={styles.pick}
            disabled={sending}
            onClick={() => filesRef.current?.click()}
          >
            <Paperclip size={22} strokeWidth={1.6} />
            {t("选文件")}
          </button>
          {/* `capture` opens the camera straight away; one shot per tap, so
              taking three receipts is three taps and one batch. */}
          <input
            ref={cameraRef}
            type="file"
            accept="image/*"
            capture="environment"
            hidden
            onChange={(e) => {
              add(e.target.files);
              e.target.value = "";
            }}
          />
          <input
            ref={filesRef}
            type="file"
            multiple
            hidden
            onChange={(e) => {
              add(e.target.files);
              e.target.value = "";
            }}
          />
        </div>

        {picked.length > 0 ? (
          <div className={styles.picked} data-picked={picked.length}>
            {picked.map((p) => (
              <div key={p.key} className={styles.file} data-too-big={p.file.size > MAX_LIBRARY_UPLOAD_BYTES}>
                {p.preview ? (
                  <img className={styles.thumb} src={p.preview} alt="" />
                ) : (
                  <span className={styles.thumb}>
                    <FileText size={18} strokeWidth={1.5} />
                  </span>
                )}
                <span className={styles.fileBody}>
                  <span className={styles.fileName}>{p.file.name || t("照片")}</span>
                  <span className={styles.fileMeta}>
                    {p.file.size > MAX_LIBRARY_UPLOAD_BYTES
                      ? t("超过 512 MB，不会上传")
                      : formatBytes(p.file.size)}
                  </span>
                </span>
                {!sending && (
                  <button
                    type="button"
                    className={styles.remove}
                    onClick={() => remove(p.key)}
                    aria-label={t("移除")}
                  >
                    <X size={15} />
                  </button>
                )}
              </div>
            ))}
          </div>
        ) : (
          <div className={treeStyles.sheetText}>
            {t("拍发票、收据，或选 PDF、文档、压缩包。AI 会读内容、起标题，并归到项目的目录里；不合适可以撤回或移动。")}
          </div>
        )}

        {sending && progress && (
          <div className={styles.sending}>
            <span className={styles.sendingText}>
              {t("正在上传 {0}/{1} · {2}%", Math.min(progress.file + 1, progress.files), progress.files, pct)}
            </span>
            <span className={styles.bar}>
              <span className={styles.barFill} style={{ width: `${Math.max(pct, 3)}%` }} />
            </span>
          </div>
        )}
        {error && <div className={treeStyles.error}>{error}</div>}

        <button
          type="button"
          className={treeStyles.primary}
          disabled={!canSend}
          onClick={() => void send()}
          data-upload-send
        >
          {sending && <Spinner size={13} />}
          {!project
            ? t("先选项目")
            : picked.length === 0
              ? t("上传")
              : t("上传 {0} 个文件（{1}）", picked.length - tooBig, formatBytes(totalBytes))}
        </button>
      </div>
    </div>
  );
}
