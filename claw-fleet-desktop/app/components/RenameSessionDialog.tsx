import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./RenameSessionDialog.module.css";

interface Props {
  /** Title currently shown for the session (auto-derived or already overridden). */
  currentTitle: string;
  /** True when an override is already set — enables the "reset to auto" action. */
  hasOverride: boolean;
  /** Save the trimmed input. An empty string clears the override (reset to auto). */
  onSave: (title: string) => void;
  onCancel: () => void;
}

/**
 * Small modal for pinning a manual title on a session (任务页 → 右键 → 重命名).
 * Mirrors ConfirmDialog's overlay/dialog shape but with a text field. Saving an
 * empty value — or the explicit "重置为自动标题" button — clears the override so
 * the display falls back to the auto-derived title.
 */
export function RenameSessionDialog({ currentTitle, hasOverride, onSave, onCancel }: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState(currentTitle);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    // Focus + select the whole title so the human can type a replacement or edit.
    const el = inputRef.current;
    if (el) {
      el.focus();
      el.select();
    }
  }, []);

  const save = () => onSave(value.trim());

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onCancel]);

  return (
    <div className={styles.overlay} onClick={onCancel}>
      <div className={styles.dialog} onClick={(e) => e.stopPropagation()}>
        <p className={styles.title}>{t("history.rename_title", "重命名会话")}</p>
        <input
          ref={inputRef}
          className={styles.input}
          value={value}
          placeholder={t("history.rename_placeholder", "输入自定义标题")}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              save();
            }
          }}
        />
        <p className={styles.hint}>{t("history.rename_hint", "留空可恢复为自动标题")}</p>
        <div className={styles.actions}>
          {hasOverride && (
            <button
              className={`${styles.btn} ${styles.btn_reset}`}
              onClick={() => onSave("")}
            >
              {t("history.rename_reset", "重置为自动标题")}
            </button>
          )}
          <span className={styles.spacer} />
          <button className={styles.btn} onClick={onCancel}>
            {t("cancel", "取消")}
          </button>
          <button className={`${styles.btn} ${styles.btn_primary}`} onClick={save}>
            {t("save", "保存")}
          </button>
        </div>
      </div>
    </div>
  );
}
