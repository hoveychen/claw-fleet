import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./PromptDialog.module.css";

interface Props {
  title: string;
  /** Secondary line under the title — explain the format, not the action. */
  hint?: string;
  defaultValue: string;
  confirmLabel: string;
  /** Server-side rejection from the previous submit, shown under the input. */
  error?: string | null;
  onConfirm: (value: string) => void;
  onCancel: () => void;
}

export function PromptDialog({
  title,
  hint,
  defaultValue,
  confirmLabel,
  error,
  onConfirm,
  onCancel,
}: Props) {
  const { t } = useTranslation();
  const [value, setValue] = useState(defaultValue);
  const inputRef = useRef<HTMLInputElement>(null);

  // Select only the last path segment: renaming in place is the common case,
  // re-parenting means typing over the whole thing anyway.
  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.focus();
    const at = defaultValue.lastIndexOf("/");
    el.setSelectionRange(at < 0 ? 0 : at + 1, defaultValue.length);
  }, [defaultValue]);

  const trimmed = value.trim();
  const canSubmit = trimmed.length > 0 && trimmed !== defaultValue;

  const submit = () => {
    if (canSubmit) onConfirm(trimmed);
  };

  return (
    <div className={styles.overlay} onClick={onCancel}>
      <div className={styles.dialog} onClick={(e) => e.stopPropagation()}>
        <p className={styles.title}>{title}</p>
        {hint && <p className={styles.hint}>{hint}</p>}
        <input
          ref={inputRef}
          className={styles.input}
          type="text"
          value={value}
          spellCheck={false}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            // Scoped to the input rather than window: ConfirmDialog's global
            // listener would otherwise also fire for a dialog stacked over it.
            if (e.key === "Enter") submit();
            if (e.key === "Escape") onCancel();
          }}
        />
        {error && <p className={styles.error}>{error}</p>}
        <div className={styles.actions}>
          <button className={styles.btn} onClick={onCancel}>
            {t("cancel")}
          </button>
          <button
            className={`${styles.btn} ${styles.btn_primary}`}
            onClick={submit}
            disabled={!canSubmit}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
