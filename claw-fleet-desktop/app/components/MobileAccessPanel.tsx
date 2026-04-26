import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { ask } from "@tauri-apps/plugin-dialog";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { QRCodeCanvas } from "qrcode.react";
import styles from "./MobileAccessPanel.module.css";

interface MobileAccessInfo {
  running: boolean;
  port: number;
  token: string;
  tunnelUrl: string | null;
  connectedClients: number;
  cloudflaredAvailable: boolean;
  settingUp: boolean;
  error: string | null;
}

interface DownloadProgress {
  downloaded: number;
  total: number;
}

type TunnelProviderPref = "auto" | "cloudflare" | "localtunnel";

const PROVIDER_PREF_KEY = "claw-fleet.mobile.tunnelProvider";

function loadProviderPref(): TunnelProviderPref {
  const v = localStorage.getItem(PROVIDER_PREF_KEY);
  if (v === "cloudflare" || v === "localtunnel" || v === "auto") return v;
  return "auto";
}

export function MobileAccessPanel({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const [info, setInfo] = useState<MobileAccessInfo | null>(null);
  const [qrData, setQrData] = useState<string | null>(null);
  const [initialLoading, setInitialLoading] = useState(true);
  const [loading, setLoading] = useState(false);
  const [phase, setPhase] = useState<"downloading" | "tunnel" | null>(null);
  const [progress, setProgress] = useState<DownloadProgress | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [providerPref, setProviderPref] = useState<TunnelProviderPref>(loadProviderPref);

  const handleProviderPrefChange = useCallback((v: TunnelProviderPref) => {
    setProviderPref(v);
    localStorage.setItem(PROVIDER_PREF_KEY, v);
  }, []);

  // Listen for setup events.
  useEffect(() => {
    const unlistenPhase = listen<string>("mobile-access-phase", (e) => {
      setPhase(e.payload as "downloading" | "tunnel");
    });
    const unlistenProgress = listen<DownloadProgress>("mobile-access-progress", (e) => {
      setProgress(e.payload);
    });
    const unlistenReady = listen<MobileAccessInfo>("mobile-access-ready", (e) => {
      setInfo(e.payload);
      setProgress(null);
      setPhase(null);
      setLoading(false);
      if (e.payload.error) {
        setError(e.payload.error);
      }
      if (e.payload.running && e.payload.tunnelUrl) {
        invoke<string | null>("get_mobile_qr_data").then(setQrData).catch(() => {});
      }
    });
    return () => {
      unlistenPhase.then((fn) => fn());
      unlistenProgress.then((fn) => fn());
      unlistenReady.then((fn) => fn());
    };
  }, []);

  // Fetch current status on mount — restore state if setup is in progress.
  useEffect(() => {
    invoke<MobileAccessInfo>("get_mobile_access_status").then(async (data) => {
      setInfo(data);
      if (data.settingUp) {
        setLoading(true);
        setPhase("downloading");
      }
      if (data.running && data.tunnelUrl) {
        const qr = await invoke<string | null>("get_mobile_qr_data").catch(() => null);
        setQrData(qr);
      }
      setInitialLoading(false);
    }).catch(() => {
      setInitialLoading(false);
    });
  }, []);

  // Poll status while panel is open (to update connected clients).
  useEffect(() => {
    if (!info?.running) return;
    const interval = setInterval(() => {
      invoke<MobileAccessInfo>("get_mobile_access_status")
        .then(setInfo)
        .catch(() => {});
    }, 5000);
    return () => clearInterval(interval);
  }, [info?.running]);

  const handleEnable = useCallback(async () => {
    setLoading(true);
    setError(null);
    setProgress(null);
    setPhase(null);
    try {
      const data = await invoke<MobileAccessInfo>("enable_mobile_access", {
        provider: providerPref === "auto" ? null : providerPref,
      });
      setInfo(data);
      const qr = await invoke<string | null>("get_mobile_qr_data");
      setQrData(qr);
    } catch (e) {
      setError(String(e));
    }
    setLoading(false);
    setProgress(null);
  }, [providerPref]);

  const handleDisable = useCallback(async () => {
    const confirmed = await ask(t("mobile_panel.confirm_disable"), { kind: "warning" });
    if (!confirmed) return;
    invoke("disable_mobile_access").catch(() => {});
    setInfo(null);
    setQrData(null);
    setError(null);
  }, [t]);

  return (
    <div className={styles.overlay} onClick={onClose}>
      <div className={styles.panel} onClick={(e) => e.stopPropagation()}>
        <div className={styles.header}>
          <h2 className={styles.title}>{t("settings.mobile_access")}</h2>
          <button className={styles.close_btn} onClick={onClose}>&times;</button>
        </div>

        <div className={styles.body}>
          {initialLoading ? (
            <div className={styles.center}>
              <p className={styles.desc}>{t("account.loading")}</p>
            </div>
          ) : !info?.running && !loading ? (
            /* ── Not running ── */
            <div className={styles.center}>
              <div className={styles.icon}>
                <svg viewBox="0 0 48 48" width="48" height="48" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <rect x="12" y="4" width="24" height="40" rx="3" />
                  <line x1="20" y1="36" x2="28" y2="36" />
                </svg>
              </div>
              <p className={styles.desc}>{t("settings.mobile_desc")}</p>
              <label className={styles.provider_row}>
                <span className={styles.provider_label}>{t("mobile_panel.provider_label")}</span>
                <select
                  className={styles.provider_select}
                  value={providerPref}
                  onChange={(e) => handleProviderPrefChange(e.target.value as TunnelProviderPref)}
                >
                  <option value="auto">{t("mobile_panel.provider_auto")}</option>
                  <option value="cloudflare">{t("mobile_panel.provider_cloudflare")}</option>
                  <option value="localtunnel">{t("mobile_panel.provider_localtunnel")}</option>
                </select>
              </label>
              <button
                className={styles.enable_btn}
                onClick={handleEnable}
                disabled={loading}
              >
                {t("settings.mobile_enable")}
              </button>
              {error && <p className={styles.error}>{error}</p>}
            </div>
          ) : loading && !qrData ? (
            /* ── Setting up (downloading / starting tunnel) ── */
            <div className={styles.center}>
              <div className={styles.icon}>
                <svg viewBox="0 0 48 48" width="48" height="48" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round">
                  <rect x="12" y="4" width="24" height="40" rx="3" />
                  <line x1="20" y1="36" x2="28" y2="36" />
                </svg>
              </div>
              <p className={styles.desc}>
                {phase === "tunnel"
                  ? t("mobile_panel.starting_tunnel")
                  : t("mobile_panel.downloading")}
              </p>
              {progress && progress.total > 0 && (
                <div className={styles.progress_container}>
                  <div className={styles.progress_bar}>
                    <div
                      className={styles.progress_fill}
                      style={{ width: `${Math.round((progress.downloaded / progress.total) * 100)}%` }}
                    />
                  </div>
                  <span className={styles.progress_text}>
                    {Math.round(progress.downloaded / 1024 / 1024)}MB / {Math.round(progress.total / 1024 / 1024)}MB
                  </span>
                </div>
              )}
              {error && <p className={styles.error}>{error}</p>}
            </div>
          ) : (
            /* ── Running ── */
            <div className={styles.center}>
              {qrData ? (
                <>
                  <p className={styles.scan_hint}>
                    {t("mobile_panel.scan_hint")}
                  </p>
                  <div className={styles.qr_container}>
                    <QRCodeCanvas
                      value={qrData}
                      size={220}
                      bgColor="#ffffff"
                      fgColor="#000000"
                      level="M"
                    />
                  </div>
                  <div className={styles.status_row}>
                    <span className={styles.status_dot} />
                    <span className={styles.status_text}>
                      {t("mobile_panel.active")}
                    </span>
                  </div>
                  {(info?.connectedClients ?? 0) > 0 && (
                    <p className={styles.clients}>
                      {info!.connectedClients} {t("mobile_panel.connected_devices")}
                    </p>
                  )}
                  {info?.tunnelUrl && (
                    <p className={styles.url}>{info.tunnelUrl}</p>
                  )}
                </>
              ) : (
                <>
                  <p className={styles.error}>
                    {info?.error || t("mobile_panel.tunnel_failed")}
                  </p>
                  <button
                    className={styles.enable_btn}
                    onClick={handleEnable}
                    disabled={loading}
                  >
                    {t("mobile_panel.retry")}
                  </button>
                </>
              )}

              <button className={styles.disable_btn} onClick={handleDisable}>
                {t("mobile_panel.stop_access")}
              </button>

              {error && <p className={styles.error}>{error}</p>}
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
