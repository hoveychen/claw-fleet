import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { CopyButton } from "./CopyButton";
import { PageShell } from "./PageShell";
import { useUIStore } from "../store";
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

/** 直连现状(claw-fleet-core::direct_host::DirectHostStatus)。 */
interface DirectHostStatus {
  baseUrl: string;
  /** 地址的问题:empty / noScheme / noHost / plainHttp / loopback,null = 没问题。 */
  problem: string | null;
  tokenPresent: boolean;
  /** token 是手填的还是取自本机 ~/.fleet/token。 */
  tokenManual: boolean;
  ready: boolean;
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

/** 「移动端」板块 — 启用 mobile relay 通道并展示配对 QR code。 */
export function MobileView() {
  const { t, i18n } = useTranslation();
  const [config, setConfig] = useState<MobileRelayConfig | null>(null);
  const [status, setStatus] = useState<MobileRelayStatus | null>(null);
  // 直连(手机不经中转,直接问这台主机的 HTTP 数据面)。与中转那半边并列,因为
  // 它是另一种设备而不是同一种设备的另一种连法。
  const [direct, setDirect] = useState<DirectHostStatus | null>(null);
  const [directQr, setDirectQr] = useState<string | null>(null);
  const [directUrl, setDirectUrl] = useState<string | null>(null);
  const [directUrlDraft, setDirectUrlDraft] = useState("");
  const [directTokenDraft, setDirectTokenDraft] = useState("");
  const [directOpen, setDirectOpen] = useState(false);
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

  /** 拉一遍直连现状,现状说能出码就把码与链接一起取回来。
   *
   *  出不了码时**清掉**上一次的码 —— 一张连不上的码留在界面上,比没有码更糟。 */
  const refreshDirect = useCallback(async () => {
    let st: DirectHostStatus | null = null;
    try {
      st = await invoke<DirectHostStatus>("direct_host_status");
    } catch {
      // 远端 backend 太旧、或这条能力不可用 —— 整块隐掉,不摆一个点不动的入口。
      setDirect(null);
      setDirectQr(null);
      setDirectUrl(null);
      return;
    }
    setDirect(st);
    setDirectUrlDraft(st.baseUrl);
    if (!st.ready) {
      setDirectQr(null);
      setDirectUrl(null);
      return;
    }
    // 码与链接各自独立取:一个失败不该把另一个也抹掉,任一单独就够加一台设备。
    try {
      setDirectQr(await invoke<string>("direct_host_qr_svg"));
    } catch {
      setDirectQr(null);
    }
    try {
      setDirectUrl(await invoke<string>("direct_host_url"));
    } catch {
      setDirectUrl(null);
    }
  }, []);

  const load = useCallback(async () => {
    void refreshDirect();
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
  }, [refreshQr, refreshDirect]);

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
                    <span className={styles.deviceLabel}>{d.label || d.platform}</span>
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

            {/* ── 直连(手机不经中转)───────────────────────────────────
                另一种设备,不是同一种设备的另一种连法:手机把这台主机的 HTTP
                数据面直接加进设备簿,省掉中转一跳,代价是没有推送通道。
                地址必须由人给 —— 这台机器不知道自己在手机那边叫什么(serve 绑在
                127.0.0.1 上,对外那一层只有部署它的人知道)。 */}
            {direct && (
              <div className={styles.directBlock}>
                <button
                  className={styles.directHead}
                  onClick={() => setDirectOpen((v) => !v)}
                >
                  <span className={styles.directTitle}>
                    {t("mobile_direct_title", "直连(不经中转)")}
                  </span>
                  <span className={styles.directState} data-ready={direct.ready ? "yes" : "no"}>
                    {direct.ready
                      ? t("mobile_direct_ready", "可出码")
                      : t("mobile_direct_not_ready", "未就绪")}
                  </span>
                </button>
                {directOpen && (
                  <div className={styles.directBody}>
                    <p className={styles.qrHint}>
                      {t(
                        "mobile_direct_hint",
                        "手机扫这张码就把「地址 + token」一起加成一台直连设备。填的是另一台已经部署好的 Fleet 主机（云容器、或反代后面那台）能被手机访问到的地址，加上它的 admin token。地址必须是 https —— 明文 http 会被手机浏览器拦掉。",
                      )}
                    </p>
                    <div className={styles.fieldRow}>
                      <span className={styles.fieldLabel}>
                        {t("mobile_direct_url", "对外地址")}
                      </span>
                      <input
                        className={styles.urlInput}
                        value={directUrlDraft}
                        placeholder="https://fleet.example.com"
                        spellCheck={false}
                        onChange={(e) => setDirectUrlDraft(e.target.value)}
                      />
                    </div>
                    <div className={styles.fieldRow}>
                      <span className={styles.fieldLabel}>
                        {t("mobile_direct_token", "token")}
                      </span>
                      <input
                        className={styles.urlInput}
                        value={directTokenDraft}
                        placeholder={
                          direct.tokenPresent && !direct.tokenManual
                            ? t(
                                "mobile_direct_token_local",
                                "留空 = 用本机 ~/.fleet/token（只有本机正跑着 serve 时才有）",
                              )
                            : t("mobile_direct_token_needed", "填那台主机的 admin token")
                        }
                        spellCheck={false}
                        onChange={(e) => setDirectTokenDraft(e.target.value)}
                      />
                      <button
                        className={styles.smallButton}
                        disabled={busy}
                        onClick={() => {
                          setBusy(true);
                          void invoke<DirectHostStatus>("set_direct_host_config", {
                            baseUrl: directUrlDraft.trim(),
                            token: directTokenDraft.trim(),
                          })
                            .then(() => refreshDirect())
                            .catch((e) => setError(String(e)))
                            .finally(() => setBusy(false));
                        }}
                      >
                        {t("save", "保存")}
                      </button>
                    </div>
                    {/* 每种「未就绪」都给出那一句能照着做的话 —— 合成一句
                        「未就绪」等于让人去猜。 */}
                    {direct.problem === "plainHttp" && (
                      <div className={styles.directWarn}>
                        {t(
                          "mobile_direct_plain_http",
                          "手机上那个页面是 https 的，浏览器不允许它连明文 http —— 换成 https（隧道或反代），否则扫了也连不上。",
                        )}
                      </div>
                    )}
                    {direct.problem === "loopback" && (
                      <div className={styles.directWarn}>
                        {t(
                          "mobile_direct_loopback",
                          "这是本机回环地址，手机访问不到 —— 只在同机调试时有意义。",
                        )}
                      </div>
                    )}
                    {(direct.problem === "noScheme" || direct.problem === "noHost") && (
                      <div className={styles.directWarn}>
                        {t("mobile_direct_bad_url", "地址要写成完整的 https://… 形式。")}
                      </div>
                    )}
                    {!direct.tokenPresent && (
                      <div className={styles.directWarn}>
                        {t(
                          "mobile_direct_no_token",
                          "本机没有在跑的 serve —— ~/.fleet/token 为空是正常的，桌面端不监听 HTTP。直连要指向的是另一台已经部署好的 Fleet 主机：填它的地址与 admin token（云容器就是 FLEET_ADMIN_TOKEN 那个值）。",
                        )}
                      </div>
                    )}
                    {directQr && (
                      <div className={styles.qrWrap}>
                        <div
                          className={styles.qr}
                          dangerouslySetInnerHTML={{ __html: directQr }}
                        />
                        <p className={styles.qrHint}>
                          {t(
                            "mobile_direct_qr_hint",
                            "这张码带着 token，请勿截图外传。",
                          )}
                        </p>
                        {directUrl && (
                          <div className={styles.copyRow}>
                            <CopyButton
                              text={directUrl}
                              label={t("mobile_direct_copy", "复制直连链接")}
                            />
                            <span className={styles.copyHint}>
                              {t(
                                "mobile_direct_copy_hint",
                                "扫码进不来时改用它：在手机上粘贴。",
                              )}
                            </span>
                          </div>
                        )}
                      </div>
                    )}
                  </div>
                )}
              </div>
            )}

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
