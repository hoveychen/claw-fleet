import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { PageShell } from "./PageShell";
import { useUIStore } from "../store";
import { RELAY_PRESETS, relayChoiceOf, type RelayChoice } from "../relayPresets";
import styles from "./MobileView.module.css";

interface MobileRelayConfig {
  enabled: boolean;
  relayUrl: string;
  secret: string;
}

interface MobileClientInfo {
  clientId: string;
  label: string;
  platform: string;
  pushSubscribed: boolean;
  connectedAtMs: number;
  lastSeenMs: number;
  /** Short git commit the phone's bundle was built from (mobile-web
   *  `__APP_COMMIT__`). Absent for a client that predates the field or built
   *  without a commit source (e.g. the Harmony native client). */
  appCommit?: string;
}

/** True when the phone's bundle commit is known, the desktop's own build commit
 *  is known, and they differ — i.e. the phone is running a stale deploy. Both
 *  are normalized to 7 chars so a full-vs-short SHA doesn't false-positive.
 *  Either side `unknown`/absent → not stale (we can't tell, so we don't cry). */
function isStale(deviceCommit: string | undefined, desktopCommit: string | null): boolean {
  if (!deviceCommit || !desktopCommit) return false;
  if (deviceCommit === "unknown" || desktopCommit === "unknown") return false;
  return deviceCommit.slice(0, 7) !== desktopCommit.slice(0, 7);
}

interface MobileRelayStatus {
  enabled: boolean;
  connected: boolean;
  clients: number;
  relayUrl: string;
  secretSet: boolean;
  devices?: MobileClientInfo[];
}

/** Emoji per platform key from mobile-web deviceLabel.ts — avoids shipping icon
 *  assets for a small list. */
const PLATFORM_ICON: Record<string, string> = {
  ios: "📱",
  android: "🤖",
  harmony: "🌐",
  windows: "🪟",
  macos: "💻",
  linux: "🐧",
  unknown: "❓",
};

/** Same single-unit shape as HistoryView's timeAgo, reusing its i18n keys. */
function timeAgo(ms: number, t: (k: string, opts?: Record<string, unknown>) => string): string {
  const diff = Date.now() - ms;
  if (diff < 60_000) return t("just_now");
  if (diff < 3_600_000) return t("m_ago", { n: Math.floor(diff / 60_000) });
  if (diff < 86_400_000) return t("h_ago", { n: Math.floor(diff / 3_600_000) });
  return t("d_ago", { n: Math.floor(diff / 86_400_000) });
}

/** Mobile section — enable mobile relay channel and show pairing QR code. */
export function MobileView() {
  const { t, i18n } = useTranslation();
  const [config, setConfig] = useState<MobileRelayConfig | null>(null);
  const [status, setStatus] = useState<MobileRelayStatus | null>(null);
  const [qrSvg, setQrSvg] = useState<string | null>(null);
  const { urlDraft, editingUrl } = useUIStore((s) => s.mainViewState.mobile);
  const updateMainViewState = useUIStore((s) => s.updateMainViewState);
  const setUrlDraft = (value: string) => updateMainViewState("mobile", { urlDraft: value });
  const setEditingUrl = (value: boolean) =>
    updateMainViewState("mobile", { editingUrl: value });
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  // The desktop's own build commit, compared against each phone's appCommit to
  // flag a stale mobile deploy. Fetched once — it's a compile-time constant.
  const [desktopCommit, setDesktopCommit] = useState<string | null>(null);

  const refreshQr = useCallback(async (enabled: boolean) => {
    if (!enabled) {
      setQrSvg(null);
      return;
    }
    // Carry the desktop's current UI language into the QR so a fresh scan opens
    // the phone in the same language (core accepts only "zh"/"en").
    const lang = i18n.language.startsWith("zh") ? "zh" : "en";
    try {
      setQrSvg(await invoke<string>("mobile_relay_qr_svg", { lang }));
    } catch {
      setQrSvg(null);
    }
  }, [i18n]);

  const load = useCallback(async () => {
    try {
      const cfg = await invoke<MobileRelayConfig>("get_mobile_relay_config");
      setConfig(cfg);
      if (!useUIStore.getState().mainViewState.mobile.editingUrl) {
        useUIStore.getState().updateMainViewState("mobile", { urlDraft: cfg.relayUrl });
      }
      await refreshQr(cfg.enabled && !!cfg.secret);
    } catch (e) {
      setError(String(e));
    }
  }, [refreshQr]);

  useEffect(() => {
    void load();
  }, [load]);

  // Desktop build commit — compile-time constant, fetch once. A remote backend
  // without this command just leaves it null (no stale flags shown).
  useEffect(() => {
    invoke<string>("desktop_build_commit")
      .then((c) => setDesktopCommit(c))
      .catch(() => setDesktopCommit(null));
  }, []);

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
        // Leave secret empty: backend preserves existing value or generates on first enable (works without returning plaintext)
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

  // Even before the config loads the page wears its shell — otherwise the banner
  // (and with it the window's drag region) would blink in only once the invoke
  // resolved.
  if (!config) {
    return (
      <PageShell view="mobile" title={t("mobile_title", "移动端")}>
        <div className={styles.container}>{error ?? "…"}</div>
      </PageShell>
    );
  }

  // Which dropdown entry is showing. `editingUrl` is the sticky "the user asked
  // for the custom box" bit — without it, typing a preset's own URL into the box
  // would collapse the box mid-edit.
  const storedChoice = relayChoiceOf(config.relayUrl);
  const relayChoice: RelayChoice = editingUrl ? "custom" : storedChoice;

  return (
    <PageShell view="mobile" title={t("mobile_title", "移动端")}>
      <div className={styles.container}>
      <div className={styles.panel}>
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

            {status?.connected && (status?.devices?.length ?? 0) > 0 && (
              <div className={styles.devices}>
                <div className={styles.devicesTitle}>
                  {t("mobile_devices_title", "已接入设备")}
                </div>
                {status!.devices!.map((d) => (
                  <div key={d.clientId} className={styles.device}>
                    <span className={styles.deviceIcon}>
                      {PLATFORM_ICON[d.platform] ?? PLATFORM_ICON.unknown}
                    </span>
                    <span className={styles.deviceLabel}>
                      {/* The phone sends "未知设备" as a fixed token for an unrecognised
                          UA (mobile-web deviceLabel.ts); localise it here, where the
                          reader's language is known. */}
                      {(d.label || d.platform).replace(/^未知设备/, t("mobile_device_unknown", "未知设备"))}
                    </span>
                    <span
                      className={styles.devicePush}
                      data-on={d.pushSubscribed ? "yes" : "no"}
                    >
                      {d.pushSubscribed
                        ? t("mobile_device_push_on", "已开通知")
                        : t("mobile_device_push_off", "未开通知")}
                    </span>
                    {d.appCommit && d.appCommit !== "unknown" && (
                      <code className={styles.deviceCommit} title={d.appCommit}>
                        {d.appCommit.slice(0, 7)}
                      </code>
                    )}
                    {isStale(d.appCommit, desktopCommit) && (
                      <span
                        className={styles.deviceStale}
                        title={t(
                          "mobile_device_stale_hint",
                          "手机端 bundle 落后于桌面端（桌面 {{desktop}}）。重新构建并部署 relay 让改动生效。",
                          { desktop: (desktopCommit ?? "").slice(0, 7) },
                        )}
                      >
                        {t("mobile_device_stale", "旧版本")}
                      </span>
                    )}
                    <span className={styles.deviceSince}>
                      {t("mobile_device_since", "接入于")} {timeAgo(d.connectedAtMs, t)}
                    </span>
                  </div>
                ))}
              </div>
            )}

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
              <select
                className={styles.relaySelect}
                value={relayChoice}
                disabled={busy}
                onChange={(e) => {
                  const choice = e.target.value as RelayChoice;
                  if (choice === "custom") {
                    // Seed the box with the host in force, so switching to
                    // custom is an edit of the current address, not a blank.
                    setUrlDraft(config.relayUrl);
                    setEditingUrl(true);
                    return;
                  }
                  setEditingUrl(false);
                  const preset = RELAY_PRESETS.find((p) => p.key === choice);
                  if (preset) void applyConfig({ relayUrl: preset.url });
                }}
              >
                <option value="global">
                  {t("mobile_relay_preset_global", "Global（海外默认）")}
                </option>
                <option value="cn">
                  {t("mobile_relay_preset_cn", "China-optimized（国内默认）")}
                </option>
                <option value="custom">{t("mobile_relay_preset_custom", "自定义地址…")}</option>
              </select>
            </div>

            {relayChoice === "custom" && (
              <div className={styles.fieldRow}>
                <input
                  className={styles.urlInput}
                  value={urlDraft}
                  onChange={(e) => setUrlDraft(e.target.value)}
                  placeholder="https://…"
                  spellCheck={false}
                />
                <button
                  className={styles.smallButton}
                  disabled={busy || !urlDraft.trim()}
                  onClick={() => {
                    setEditingUrl(false);
                    void applyConfig({ relayUrl: urlDraft.trim() });
                  }}
                >
                  {t("save", "保存")}
                </button>
              </div>
            )}

            {/* The hostnames the options used to spell out live here instead —
                the option labels stay short, and the host in force is still on
                screen (for "custom" the text box above already shows it). */}
            <p className={styles.relayHint}>
              {relayChoice === "cn"
                ? t("mobile_relay_hint_cn", "经大陆反向代理接入，境内网络更稳。")
                : relayChoice === "custom"
                  ? t("mobile_relay_hint_custom", "自建或自托管的 relay 地址。")
                  : t("mobile_relay_hint_global", "直连 relay 主机，不经额外中转。")}
              {relayChoice !== "custom" && (
                <>
                  {" "}
                  {t(
                    "mobile_relay_hint_shared",
                    "两个预设指向同一个 relay，只是网络路径不同；已配对的手机需重新扫码才会走新地址。",
                  )}
                  <code className={styles.relayHost}>{config.relayUrl}</code>
                </>
              )}
            </p>

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
    </PageShell>
  );
}
