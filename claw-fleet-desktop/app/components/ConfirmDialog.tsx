import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { Spinner } from "./loading";
import styles from "./ConfirmDialog.module.css";

interface Props {
  message: string;
  onConfirm: () => void;
  onCancel: () => void;
  /** The confirmed action is in flight: show a spinner on the confirm button
   *  and refuse further confirm / cancel input (buttons, Enter, Escape, the
   *  overlay click) so it cannot be submitted twice or abandoned half-way. */
  busy?: boolean;
}

export function ConfirmDialog({ message, onConfirm, onCancel, busy = false }: Props) {
  const { t } = useTranslation();

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (busy) return;
      if (e.key === "Escape") onCancel();
      if (e.key === "Enter") onConfirm();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onConfirm, onCancel, busy]);

  return (
    <div className={styles.overlay} onClick={busy ? undefined : onCancel}>
      <div
        className={styles.dialog}
        onClick={(e) => e.stopPropagation()}
        aria-busy={busy || undefined}
      >
        <p className={styles.message}>{message}</p>
        <div className={styles.actions}>
          <button className={styles.btn} onClick={onCancel} disabled={busy}>
            {t("cancel")}
          </button>
          <button
            className={`${styles.btn} ${styles.btn_danger}`}
            onClick={onConfirm}
            disabled={busy}
            autoFocus
          >
            {busy && <Spinner size={12} />}
            {t("confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
