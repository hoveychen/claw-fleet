import { useEffect } from "react";
import { useTranslation } from "react-i18next";
import { X } from "lucide-react";
import { useReportStore, useUIStore } from "../../store";
import { ReportDetail } from "./ReportView";
import styles from "./DailyReportPopup.module.css";

/**
 * The daily report, pushed at the user instead of waiting to be found.
 *
 * Raised by `useReportStore.maybePopupReport`, which the scheduler's
 * `daily-report-ready` event and the boot check both call. Deliberately an
 * in-window overlay rather than a second OS window — see the `no-extra-windows`
 * plan; the settings panel went the same way.
 */
export function DailyReportPopup() {
  const { t } = useTranslation();
  const date = useReportStore((s) => s.reportPopupDate);
  const closeReportPopup = useReportStore((s) => s.closeReportPopup);
  const setViewMode = useUIStore((s) => s.setViewMode);

  // Esc closes, same affordance as the settings overlay.
  useEffect(() => {
    if (!date) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") closeReportPopup();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [date, closeReportPopup]);

  if (!date) return null;

  return (
    <div className={styles.overlay} onClick={closeReportPopup}>
      <div className={styles.panel} onClick={(e) => e.stopPropagation()}>
        <div className={styles.header}>
          <h2 className={styles.title}>{t("report.popup_title", "今日日报已生成")}</h2>
          <div className={styles.header_actions}>
            <button
              className={styles.open_btn}
              onClick={() => {
                closeReportPopup();
                setViewMode("report");
              }}
            >
              {t("report.popup_open_full", "查看完整日报")}
            </button>
            <button
              className={styles.close_btn}
              onClick={closeReportPopup}
              aria-label={t("common.close", "关闭")}
            >
              <X size={16} strokeWidth={2} />
            </button>
          </div>
        </div>
        <div className={styles.body}>
          <ReportDetail />
        </div>
      </div>
    </div>
  );
}
