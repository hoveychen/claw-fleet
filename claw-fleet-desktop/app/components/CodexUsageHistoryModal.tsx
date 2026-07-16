import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { CodexUsageHistoryChart } from "./CodexUsageHistoryChart";
import styles from "./UsageHistoryModal.module.css";

interface Props {
  onClose: () => void;
}

export function CodexUsageHistoryModal({ onClose }: Props) {
  const { t } = useTranslation();

  // Close on Escape
  useEffect(() => {
    const handler = (e: KeyboardEvent) => {
      if (e.key === "Escape") onClose();
    };
    window.addEventListener("keydown", handler);
    return () => window.removeEventListener("keydown", handler);
  }, [onClose]);

  return (
    <div className={styles.overlay} onClick={onClose}>
      <div className={styles.modal} onClick={(e) => e.stopPropagation()}>
        <div className={styles.header}>
          <span className={styles.title}>{t("account.occupancy_title")}</span>
          <button className={styles.close_btn} onClick={onClose}>
            ✕
          </button>
        </div>
        <div className={styles.body}>
          <CodexUsageHistoryChart height={360} />
        </div>
      </div>
    </div>
  );
}
