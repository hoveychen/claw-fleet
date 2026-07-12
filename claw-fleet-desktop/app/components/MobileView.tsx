import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import styles from "./MobileView.module.css";

interface MobileRelayConfig {
  enabled: boolean;
  relayUrl: string;
  secret: string;
}

interface MobileRelayStatus {
  enabled: boolean;
  connected: boolean;
  clients: number;
  relayUrl: string;
  secretSet: boolean;
}

/** 「移动端」板块 — 启用 mobile relay 通道并展示配对 QR code。 */
export function MobileView() {
  const { t } = useTranslation();
  const [config, setConfig] = useState<MobileRelayConfig | null>(null);
  const [status, setStatus] = useState<MobileRelayStatus | null>(null);
  const [qrSvg, setQrSvg] = useState<string | null>(null);
  const [urlDraft, setUrlDraft] = useState("");
  const [editingUrl, setEditingUrl] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const refreshQr = useCallback(async (enabled: boolean) => {
    if (!enabled) {
      setQrSvg(null);
      return;
    }
    try {
      setQrSvg(await invoke<string>("mobile_relay_qr_svg"));
    } catch {
      setQrSvg(null);
    }
  }, []);

  const load = useCallback(async () => {
    try {
      const cfg = await invoke<MobileRelayConfig>("get_mobile_relay_config");
      setConfig(cfg);
      setUrlDraft(cfg.relayUrl);
      await refreshQr(cfg.enabled && !!cfg.secret);
    } catch (e) {
      setError(String(e));
    }
  }, [refreshQr]);

  useEffect(() => {
    void load();
  }, [load]);

  // Status poll while the view is open.
  useEffect(() => {
    let alive = true;
    const tick = async () => {
      try {
        const s = await invoke<MobileRelayStatus>("mobile_relay_status");
        if (alive) setStatus(s);
      } catch {
        /* remote backend without mobile relay support */
      }
    };
    void tick();
    const timer = window.setInterval(tick, 3000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, []);

  const applyConfig = useCallback(
    async (next: Partial<MobileRelayConfig>) => {
      if (!config) return;
      setBusy(true);
      setError(null);
      try {
        // secret 留空：后端保留现有值或首次启用时生成（不回传明文也能工作）
        const stored = await invoke<MobileRelayConfig>("set_mobile_relay_config", {
          cfg: { ...config, ...next },
        });
        setConfig(stored);
        setUrlDraft(stored.relayUrl);
        await refreshQr(stored.enabled && !!stored.secret);
      } catch (e) {
        setError(String(e));
      } finally {
        setBusy(false);
      }
    },
    [config, refreshQr],
  );

  const rotate = useCallback(async () => {
    setBusy(true);
    setError(null);
    try {
      const stored = await invoke<MobileRelayConfig>("rotate_mobile_relay_secret");
      setConfig(stored);
      await refreshQr(stored.enabled && !!stored.secret);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }, [refreshQr]);

  if (!config) {
    return <div className={styles.container}>{error ?? "…"}</div>;
  }

  return (
    <div className={styles.container}>
      <div className={styles.panel}>
        <h2 className={styles.title}>{t("mobile_title", "移动端")}</h2>
        <p className={styles.subtitle}>
          {t(
            "mobile_subtitle",
            "手机扫码打开 mobile web，随时处理决策卡、查看任务进度，并通过通知第一时间收到新决策。",
          )}
        </p>

        <label className={styles.toggleRow}>
          <span>{t("mobile_enable", "启用移动端通道")}</span>
          <input
            type="checkbox"
            checked={config.enabled}
            disabled={busy}
            onChange={(e) => void applyConfig({ enabled: e.target.checked })}
          />
        </label>

        {config.enabled && (
          <>
            <div className={styles.statusRow}>
              <span
                className={styles.dot}
                data-state={status?.connected ? "on" : "off"}
              />
              <span>
                {status?.connected
                  ? t("mobile_connected", "已连接 relay")
                  : t("mobile_disconnected", "未连接 relay（检查 relay 地址或网络）")}
              </span>
              {status?.connected && (
                <span className={styles.clients}>
                  {t("mobile_clients", "{{count}} 台手机在线", { count: status?.clients ?? 0 })}
                </span>
              )}
            </div>

            {qrSvg ? (
              <div className={styles.qrWrap}>
                <div className={styles.qr} dangerouslySetInnerHTML={{ __html: qrSvg }} />
                <p className={styles.qrHint}>
                  {t(
                    "mobile_qr_hint",
                    "用手机相机扫码打开。二维码包含配对密钥，请勿截图外传。iPhone 需在 Safari 中「添加到主屏幕」后才能收到推送通知。",
                  )}
                </p>
              </div>
            ) : (
              <div className={styles.qrWrap}>
                <div className={styles.qrPlaceholder}>QR</div>
              </div>
            )}

            <div className={styles.fieldRow}>
              <span className={styles.fieldLabel}>{t("mobile_relay_url", "Relay 地址")}</span>
              {editingUrl ? (
                <>
                  <input
                    className={styles.urlInput}
                    value={urlDraft}
                    onChange={(e) => setUrlDraft(e.target.value)}
                    spellCheck={false}
                  />
                  <button
                    className={styles.smallButton}
                    disabled={busy}
                    onClick={() => {
                      setEditingUrl(false);
                      void applyConfig({ relayUrl: urlDraft.trim() });
                    }}
                  >
                    {t("save", "保存")}
                  </button>
                </>
              ) : (
                <>
                  <code className={styles.urlValue}>{config.relayUrl}</code>
                  <button className={styles.smallButton} onClick={() => setEditingUrl(true)}>
                    {t("edit", "编辑")}
                  </button>
                </>
              )}
            </div>

            <div className={styles.dangerZone}>
              <button className={styles.dangerButton} disabled={busy} onClick={() => void rotate()}>
                {t("mobile_rotate", "重新生成配对密钥")}
              </button>
              <span className={styles.dangerHint}>
                {t("mobile_rotate_hint", "旧二维码与已配对的手机将立即失效。")}
              </span>
            </div>
          </>
        )}

        {error && <div className={styles.error}>{error}</div>}
      </div>
    </div>
  );
}
