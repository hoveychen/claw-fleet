import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { getItem, setItem } from "../storage";
import styles from "./HooksBanner.module.css";

interface HookSetupPlan {
  toAdd: string[];
  hooksGloballyDisabled: boolean;
  alreadyInstalled: boolean;
}

const DISMISSED_KEY = "hooks-banner-dismissed";

export function HooksBanner() {
  const { t } = useTranslation();
  const [plan, setPlan] = useState<HookSetupPlan | null>(null);
  const [showConfirm, setShowConfirm] = useState(false);
  const [dismissed, setDismissed] = useState(() => !!getItem(DISMISSED_KEY));
  const [status, setStatus] = useState<"idle" | "success" | "error">("idle");
  const [errorMsg, setErrorMsg] = useState("");

  useEffect(() => {
    if (dismissed) return;
    invoke<HookSetupPlan>("get_hooks_setup_plan").then(setPlan).catch(() => {});
  }, [dismissed]);

  const handleInstall = useCallback(async () => {
    try {
      await invoke("apply_hooks_setup");
      setStatus("success");
      setShowConfirm(false);
      // Auto-dismiss after 3s
      setTimeout(() => setDismissed(true), 3000);
    } catch (e) {
      setStatus("error");
      setErrorMsg(String(e));
    }
  }, []);

  const handleDismiss = useCallback(() => {
    setDismissed(true);
    setItem(DISMISSED_KEY, "1");
  }, []);

  // Don't show if already installed, dismissed, or no plan
  if (dismissed || !plan || plan.alreadyInstalled) return null;

  if (status === "success") {
    return (
      <div className={`${styles.banner} ${styles.success}`}>
        <span>{t("hooks.installed")}</span>
      </div>
    );
  }

  return (
    <>
      <div className={styles.banner}>
        <span className={styles.text}>{t("hooks.banner")}</span>
        <div className={styles.actions}>
          <button className={styles.install_btn} onClick={() => setShowConfirm(true)}>
            {t("hooks.install")}
          </button>
          <button className={styles.dismiss_btn} onClick={handleDismiss}>
            {t("hooks.dismiss")}
          </button>
        </div>
      </div>

      {showConfirm && (
        <div className={styles.overlay} onClick={() => setShowConfirm(false)}>
          <div className={styles.dialog} onClick={(e) => e.stopPropagation()}>
            <h3 className={styles.dialog_title}>{t("hooks.confirm_title")}</h3>
            <p className={styles.dialog_body}>
              {t("hooks.confirm_body", { events: plan.toAdd.join(", ") })}
              {plan.hooksGloballyDisabled && t("hooks.confirm_body_disabled_warning")}
            </p>
            {status === "error" && (
              <p className={styles.error}>{t("hooks.install_error", { error: errorMsg })}</p>
            )}
            <div className={styles.dialog_actions}>
              <button className={styles.cancel_btn} onClick={() => setShowConfirm(false)}>
                {t("cancel")}
              </button>
              <button className={styles.confirm_btn} onClick={handleInstall}>
                {t("hooks.install")}
              </button>
            </div>
          </div>
        </div>
      )}
    </>
  );
}
