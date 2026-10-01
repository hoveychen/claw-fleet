import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { Spinner } from "./loading";
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
  /** The submitted value is being applied: spinner on the confirm button,
   *  input and both buttons locked, Enter / Escape / overlay click ignored. */
  busy?: boolean;
}

export function PromptDialog({
  title,
  hint,
  defaultValue,
  confirmLabel,
  error,
  onConfirm,
  onCancel,
  busy = false,
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
    if (canSubmit && !busy) onConfirm(trimmed);
  };
  const cancel = () => {
    if (!busy) onCancel();
  };

  return (
    <div className={styles.overlay} onClick={cancel}>
      <div
        className={styles.dialog}
        onClick={(e) => e.stopPropagation()}
        aria-busy={busy || undefined}
      >
        <p className={styles.title}>{title}</p>
        {hint && <p className={styles.hint}>{hint}</p>}
        <input
          ref={inputRef}
          className={styles.input}
          type="text"
          value={value}
          spellCheck={false}
          readOnly={busy}
          onChange={(e) => setValue(e.target.value)}
          onKeyDown={(e) => {
            // Scoped to the input rather than window: ConfirmDialog's global
            // listener would otherwise also fire for a dialog stacked over it.
            if (e.key === "Enter") submit();
            if (e.key === "Escape") cancel();
          }}
        />
        {error && <p className={styles.error}>{error}</p>}
        <div className={styles.actions}>
          <button className={styles.btn} onClick={cancel} disabled={busy}>
            {t("cancel")}
          </button>
          <button
            className={`${styles.btn} ${styles.btn_primary}`}
            onClick={submit}
            disabled={!canSubmit || busy}
          >
            {busy && <Spinner size={12} />}
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
