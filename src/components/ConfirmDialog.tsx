import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import styles from "./ConfirmDialog.module.css";

interface Props {
  message: string;
  onConfirm: () => void;
  onCancel: () => void;
}

export function ConfirmDialog({ message, onConfirm, onCancel }: Props) {
  const { t } = useTranslation();

  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
      if (e.key === "Enter") onConfirm();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onConfirm, onCancel]);

  return (
    <div className={styles.overlay} onClick={onCancel}>
      <div className={styles.dialog} onClick={(e) => e.stopPropagation()}>
        <p className={styles.message}>{message}</p>
        <div className={styles.actions}>
          <button className={styles.btn} onClick={onCancel}>
            {t("cancel")}
          </button>
          <button className={`${styles.btn} ${styles.btn_danger}`} onClick={onConfirm} autoFocus>
            {t("confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
