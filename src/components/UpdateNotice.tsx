import { useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import { getItem } from "../storage";
import styles from "./UpdateNotice.module.css";

interface VersionCheckResult {
  current_version: string;
  latest_version: string;
  has_update: boolean;
  release_url: string;
}

export function UpdateNotice() {
  const { t } = useTranslation();
  const [result, setResult] = useState<VersionCheckResult | null>(null);
  const [dismissed, setDismissed] = useState(false);

  useEffect(() => {
    if (getItem("auto-update-check") === "false") return;
    invoke<VersionCheckResult>("check_app_version").then(setResult).catch(() => {});
  }, []);

  if (!result?.has_update || dismissed) return null;

  return (
    <div className={styles.banner}>
      <span className={styles.text}>
        {t("update.available", { version: result.latest_version })}
      </span>
      <div className={styles.actions}>
        <button
          className={`${styles.btn} ${styles.btn_primary}`}
          onClick={() => openUrl(result.release_url).catch(() => {})}
        >
          {t("update.update_now")}
        </button>
        <button className={styles.btn} onClick={() => setDismissed(true)}>
          {t("update.later")}
        </button>
      </div>
    </div>
  );
}
