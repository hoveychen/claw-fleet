import { invoke } from "@tauri-apps/api/core";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useConnectionStore, useDetailStore, useOverlayStore } from "../store";
import { getItem, setItem } from "../storage";
import { playChime, speakText, getVoices, CHIME_PRESETS, type ChimePreset, type TtsVoice } from "../audio";
import { QRCodeCanvas } from "qrcode.react";
import { AccountInfo } from "./AccountInfo";
import { LanguageSwitcher } from "./LanguageSwitcher";
import { ThemeToggle } from "./ThemeToggle";
import { AgentSourceIcon } from "./SessionCard";
import styles from "./SettingsPanel.module.css";

interface HookSetupPlan {
  toAdd: string[];
  hooksGloballyDisabled: boolean;
  alreadyInstalled: boolean;
}

interface SourceInfo {
  name: string;
  enabled: boolean;
  available: boolean;
}

interface LlmModel {
  id: string;
  displayName: string;
}

interface LlmProviderInfo {
  name: string;
  displayName: string;
  available: boolean;
  models: LlmModel[];
  defaultFastModel: string;
  defaultStandardModel: string;
}

interface LlmConfig {
  provider: string;
  fastModel: string;
  standardModel: string;
}

type NotificationMode = "all" | "user_action" | "none";
type TtsMode = "chime_and_speech" | "chime_only" | "off";
type SettingsTab = "general" | "appearance" | "profile" | "connection" | "mobile" | "notifications" | "sound";

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

const tabIcons: Record<SettingsTab, React.ReactNode> = {
  general: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="8" cy="8" r="1.5" />
      <path d="M6.7 1.2l-.4 1.6a5 5 0 0 0-1.5.9L3.3 3.2 1.9 5.6l1.2 1.1a5 5 0 0 0 0 1.7l-1.2 1.1 1.4 2.4 1.5-.5a5 5 0 0 0 1.5.9l.4 1.6h2.6l.4-1.6a5 5 0 0 0 1.5-.9l1.5.5 1.4-2.4-1.2-1.1a5 5 0 0 0 0-1.7l1.2-1.1-1.4-2.4-1.5.5a5 5 0 0 0-1.5-.9L9.3 1.2z" />
    </svg>
  ),
  appearance: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="8" cy="8" r="3" />
      <path d="M8 1v2M8 13v2M1 8h2M13 8h2M3.05 3.05l1.41 1.41M11.54 11.54l1.41 1.41M3.05 12.95l1.41-1.41M11.54 4.46l1.41-1.41" />
    </svg>
  ),
  profile: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="8" cy="5" r="3" />
      <path d="M2.5 14a5.5 5.5 0 0 1 11 0" />
    </svg>
  ),
  connection: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M6 10l4-4" />
      <path d="M9.5 3.5l1-1a2.12 2.12 0 0 1 3 3l-1 1M6.5 12.5l-1 1a2.12 2.12 0 0 1-3-3l1-1" />
    </svg>
  ),
  mobile: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <rect x="4" y="1" width="8" height="14" rx="1.5" />
      <line x1="7" y1="12" x2="9" y2="12" />
    </svg>
  ),
  notifications: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M6 13a2 2 0 0 0 4 0" />
      <path d="M12 7c0-2.76-1.79-5-4-5S4 4.24 4 7c0 3-1.5 4.5-2 5h12c-.5-.5-2-2-2-5z" />
    </svg>
  ),
  sound: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M8 2L4 6H1v4h3l4 4V2z" />
      <path d="M11 5.5a3.5 3.5 0 0 1 0 5M13 3.5a6.5 6.5 0 0 1 0 9" />
    </svg>
  ),
};

export function SettingsPanel({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { connection, disconnect } = useConnectionStore();
  const [activeTab, setActiveTab] = useState<SettingsTab>("general");

  // ── Sources state ────────────────────────────────────────────────────────
  const [sources, setSources] = useState<SourceInfo[]>([]);
  const [sourcesNeedRestart, setSourcesNeedRestart] = useState(false);

  useEffect(() => {
    invoke<SourceInfo[]>("get_sources_config").then(setSources).catch(() => {});
  }, []);

  const handleToggleSource = useCallback(async (name: string, enabled: boolean) => {
    try {
      await invoke("set_source_enabled", { name, enabled });
      setSources((prev) => prev.map((s) => (s.name === name ? { ...s, enabled } : s)));
      setSourcesNeedRestart(true);
    } catch {
      // ignore
    }
  }, []);

  // ── Hooks state ──────────────────────────────────────────────────────────
  const [hooksPlan, setHooksPlan] = useState<HookSetupPlan | null>(null);
  const [hooksStatus, setHooksStatus] = useState<"idle" | "installing" | "success" | "error">("idle");
  const [hooksError, setHooksError] = useState("");

  useEffect(() => {
    invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
  }, []);

  const handleInstallHooks = useCallback(async () => {
    setHooksStatus("installing");
    try {
      await invoke("apply_hooks_setup");
      setHooksStatus("success");
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      setHooksStatus("error");
      setHooksError(String(e));
    }
  }, []);

  // ── Notifications state ─────────────────────────────────────────────────
  const [notifMode, setNotifMode] = useState<NotificationMode>(
    () => (getItem("notification-mode") as NotificationMode) || "user_action",
  );
  const [notifPermission, setNotifPermission] = useState<boolean | null>(null);

  useEffect(() => {
    isPermissionGranted().then(setNotifPermission).catch(() => {});
  }, []);

  const handleNotifModeChange = useCallback((mode: NotificationMode) => {
    setNotifMode(mode);
    setItem("notification-mode", mode);
    invoke("set_notification_mode", { mode }).catch(() => {});
  }, []);

  const handleRequestPermission = useCallback(async () => {
    const result = await requestPermission();
    if (result === "granted") {
      setNotifPermission(true);
    } else {
      // Permission denied — open system settings
      invoke("open_notification_settings").catch(() => {});
    }
  }, []);

  // ── TTS state ──────────────────────────────────────────────────────────
  const [ttsMode, setTtsMode] = useState<TtsMode>(
    () => (getItem("tts-mode") as TtsMode) || "off",
  );

  const handleTtsModeChange = useCallback((mode: TtsMode) => {
    setTtsMode(mode);
    setItem("tts-mode", mode);
  }, []);

  // ── Chime preset state ────────────────────────────────────────────────
  const [chimePreset, setChimePreset] = useState<ChimePreset>(
    () => (getItem("chime-sound") as ChimePreset) || "ding_dong",
  );

  const handleChimeChange = useCallback((preset: ChimePreset) => {
    setChimePreset(preset);
    setItem("chime-sound", preset);
    playChime(preset);
  }, []);

  // ── TTS voice state ───────────────────────────────────────────────────
  const [ttsVoice, setTtsVoice] = useState(() => getItem("tts-voice") || "");
  const [voices, setVoices] = useState<TtsVoice[]>([]);

  useEffect(() => {
    getVoices().then(setVoices);
  }, []);

  const handleVoiceChange = useCallback((uri: string) => {
    setTtsVoice(uri);
    setItem("tts-voice", uri);
  }, []);

  // Sync notification mode to backend on mount
  useEffect(() => {
    invoke("set_notification_mode", { mode: notifMode }).catch(() => {});
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  // ── User title state ───────────────────────────────────────────────────
  const [userTitle, setUserTitle] = useState(() => getItem("user-title") || "");

  const handleUserTitleChange = useCallback((title: string) => {
    setUserTitle(title);
    setItem("user-title", title);
    invoke("set_user_title", { title }).catch(() => {});
  }, []);

  // ── Personalized mascot state ──────────────────────────────────────────
  const [personalizedMascot, setPersonalizedMascot] = useState(
    () => getItem("personalized-mascot") !== "false",
  );

  const handleTogglePersonalizedMascot = useCallback((enabled: boolean) => {
    setPersonalizedMascot(enabled);
    setItem("personalized-mascot", enabled ? "true" : "false");
  }, []);

  // ── Auto update check state ────────────────────────────────────────────
  const [autoUpdateCheck, setAutoUpdateCheck] = useState(
    () => getItem("auto-update-check") !== "false",
  );

  const handleToggleAutoUpdateCheck = useCallback((enabled: boolean) => {
    setAutoUpdateCheck(enabled);
    setItem("auto-update-check", enabled ? "true" : "false");
  }, []);

  // ── LLM provider state ──────────────────────────────────────────────────
  const [llmProviders, setLlmProviders] = useState<LlmProviderInfo[]>([]);
  const [llmConfig, setLlmConfigState] = useState<LlmConfig>(() => ({
    provider: getItem("llm-provider") || "claude",
    fastModel: getItem("llm-model-fast") || "haiku",
    standardModel: getItem("llm-model-standard") || "sonnet",
  }));

  useEffect(() => {
    invoke<LlmProviderInfo[]>("list_llm_providers").then(setLlmProviders).catch(() => {});
    invoke<LlmConfig>("get_llm_config").then((cfg) => {
      setLlmConfigState(cfg);
      setItem("llm-provider", cfg.provider);
      setItem("llm-model-fast", cfg.fastModel);
      setItem("llm-model-standard", cfg.standardModel);
    }).catch(() => {});
  }, []);

  const handleLlmConfigChange = useCallback((patch: Partial<LlmConfig>) => {
    setLlmConfigState((prev) => {
      const next = { ...prev, ...patch };
      // When provider changes, reset models to that provider's defaults
      if (patch.provider && patch.provider !== prev.provider) {
        const info = llmProviders.find((p) => p.name === patch.provider);
        if (info) {
          next.fastModel = info.defaultFastModel;
          next.standardModel = info.defaultStandardModel;
        }
      }
      setItem("llm-provider", next.provider);
      setItem("llm-model-fast", next.fastModel);
      setItem("llm-model-standard", next.standardModel);
      invoke("set_llm_config", { config: next }).catch(() => {});
      return next;
    });
  }, [llmProviders]);

  const currentProviderInfo = llmProviders.find((p) => p.name === llmConfig.provider);

  // ── Overlay state (shared via store) ─────────────────────────────────────
  const overlayEnabled = useOverlayStore((s) => s.enabled);
  const setOverlayEnabled = useOverlayStore((s) => s.setEnabled);
  const isMacOS = document.documentElement.getAttribute("data-platform") === "macos";

  // ── Mobile access state ──────────────────────────────────────────────
  const [mobileAccess, setMobileAccess] = useState<MobileAccessInfo | null>(null);
  const [mobileLoading, setMobileLoading] = useState(false);
  const [mobileQrData, setMobileQrData] = useState<string | null>(null);

  useEffect(() => {
    invoke<MobileAccessInfo>("get_mobile_access_status").then((info) => {
      setMobileAccess(info);
      if (info.running) {
        invoke<string | null>("get_mobile_qr_data").then(setMobileQrData).catch(() => {});
      }
    }).catch(() => {});
  }, []);

  const handleEnableMobileAccess = useCallback(async () => {
    setMobileLoading(true);
    try {
      const info = await invoke<MobileAccessInfo>("enable_mobile_access");
      setMobileAccess(info);
      const qr = await invoke<string | null>("get_mobile_qr_data");
      setMobileQrData(qr);
    } catch (e) {
      console.error("Failed to enable mobile access:", e);
    }
    setMobileLoading(false);
  }, []);

  const handleDisableMobileAccess = useCallback(async () => {
    await invoke("disable_mobile_access").catch(() => {});
    setMobileAccess(null);
    setMobileQrData(null);
  }, []);

  const handleSwitchConnection = useCallback(async () => {
    await useDetailStore.getState().close();
    await disconnect();
    onClose();
  }, [disconnect, onClose]);

  const hooksInstalled = hooksPlan?.alreadyInstalled || hooksStatus === "success";

  const tabs: { key: SettingsTab; label: string }[] = [
    { key: "general", label: t("settings.general") },
    { key: "appearance", label: t("settings.appearance") },
    { key: "profile", label: t("settings.profile") },
    { key: "connection", label: t("settings.connection") },
    { key: "mobile", label: t("settings.mobile_access") },
    { key: "notifications", label: t("settings.notifications") },
    { key: "sound", label: t("settings.sound") },
  ];

  return (
    <div className={styles.overlay} onClick={onClose}>
      <div className={styles.panel} onClick={(e) => e.stopPropagation()}>
        {/* Header */}
        <div className={styles.header}>
          <h2 className={styles.title}>{t("settings.title")}</h2>
          <button className={styles.close_btn} onClick={onClose}>
            {t("settings.close")}
          </button>
        </div>

        <div className={styles.body}>
          {/* ── Left: menu ── */}
          <nav className={styles.menu}>
            {tabs.map((tab) => (
              <button
                key={tab.key}
                className={`${styles.menu_item} ${activeTab === tab.key ? styles.menu_item_active : ""}`}
                onClick={() => setActiveTab(tab.key)}
              >
                {tabIcons[tab.key]}
                {tab.label}
              </button>
            ))}
          </nav>

          {/* ── Right: content ── */}
          <div className={styles.content}>
            {/* ── General ── */}
            {activeTab === "general" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.general")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.language")}</span>
                  <LanguageSwitcher />
                </div>
                {!isMacOS && (
                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.overlay")}</span>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                      {t("settings.overlay_desc")}
                    </span>
                  </div>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={overlayEnabled}
                      onChange={(e) => setOverlayEnabled(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>
                )}
                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.auto_update_check")}</span>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                      {t("settings.auto_update_check_desc")}
                    </span>
                  </div>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={autoUpdateCheck}
                      onChange={(e) => handleToggleAutoUpdateCheck(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>

                {/* ── LLM Provider ── */}
                <div className={styles.section_title} style={{ marginTop: 18 }}>
                  {t("settings.llm_provider")}
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.llm_provider_select")}</span>
                  <select
                    className={styles.select}
                    style={{ flex: "none", width: 180 }}
                    value={llmConfig.provider}
                    onChange={(e) => handleLlmConfigChange({ provider: e.target.value })}
                  >
                    {llmProviders.map((p) => (
                      <option key={p.name} value={p.name} disabled={!p.available}>
                        {p.displayName}{!p.available ? ` (${t("settings.source_not_detected")})` : ""}
                      </option>
                    ))}
                  </select>
                </div>
                {llmConfig.provider === "none" && (
                  <div className={styles.row}>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-warning, #e8a838)" }}>
                      {t("settings.llm_disabled_warning")}
                    </span>
                  </div>
                )}
                {currentProviderInfo && currentProviderInfo.models.length > 0 && (
                  <>
                    <div className={styles.row}>
                      <div>
                        <span className={styles.row_label}>{t("settings.llm_fast_model")}</span>
                        <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                          {t("settings.llm_fast_model_desc")}
                        </span>
                      </div>
                      <select
                        className={styles.select}
                        style={{ flex: "none", width: 180 }}
                        value={llmConfig.fastModel}
                        onChange={(e) => handleLlmConfigChange({ fastModel: e.target.value })}
                      >
                        {currentProviderInfo.models.map((m) => (
                          <option key={m.id} value={m.id}>{m.displayName}</option>
                        ))}
                      </select>
                    </div>
                    <div className={styles.row}>
                      <div>
                        <span className={styles.row_label}>{t("settings.llm_standard_model")}</span>
                        <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                          {t("settings.llm_standard_model_desc")}
                        </span>
                      </div>
                      <select
                        className={styles.select}
                        style={{ flex: "none", width: 180 }}
                        value={llmConfig.standardModel}
                        onChange={(e) => handleLlmConfigChange({ standardModel: e.target.value })}
                      >
                        {currentProviderInfo.models.map((m) => (
                          <option key={m.id} value={m.id}>{m.displayName}</option>
                        ))}
                      </select>
                    </div>
                  </>
                )}
              </div>
            )}

            {/* ── Appearance ── */}
            {activeTab === "appearance" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.appearance")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.theme")}</span>
                  <ThemeToggle />
                </div>
                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.personalized_mascot")}</span>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                      {t("settings.personalized_mascot_desc")}
                    </span>
                  </div>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={personalizedMascot}
                      onChange={(e) => handleTogglePersonalizedMascot(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>
              </div>
            )}

            {/* ── Profile ── */}
            {activeTab === "profile" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.profile")}</div>
                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.user_title")}</span>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                      {t("settings.user_title_desc")}
                    </span>
                  </div>
                  <input
                    type="text"
                    className={styles.select}
                    value={userTitle}
                    placeholder={t("settings.user_title_placeholder")}
                    onChange={(e) => handleUserTitleChange(e.target.value)}
                    style={{ width: 120, textAlign: "center" }}
                  />
                </div>
                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("account.panel_title")}</div>
                <div className={styles.account_embed}>
                  <AccountInfo embedded />
                </div>
              </div>
            )}

            {/* ── Connection (merged: connection + hooks + sources) ── */}
            {activeTab === "connection" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.connection")}</div>
                <div className={styles.row}>
                  <div className={styles.connection_info}>
                    <span className={styles.row_label}>{t("settings.current_connection")}</span>
                    <span className={styles.connection_badge}>
                      {connection?.type === "remote" ? t("settings.remote") : t("settings.local")}
                    </span>
                  </div>
                  <button className={styles.switch_btn} onClick={handleSwitchConnection}>
                    {t("switch_connection")}
                  </button>
                </div>

                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.hooks")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.hooks_desc")}</span>
                </div>
                <div className={styles.row}>
                  {hooksInstalled ? (
                    <span className={styles.hooks_ok}>{t("hooks.installed")}</span>
                  ) : (
                    <div>
                      <span className={styles.hooks_warn}>{t("hooks.banner")}</span>
                    </div>
                  )}
                  {!hooksInstalled && hooksPlan && (
                    <button
                      className={styles.hooks_install_btn}
                      onClick={handleInstallHooks}
                      disabled={hooksStatus === "installing"}
                    >
                      {hooksStatus === "installing" ? t("account.loading") : t("hooks.install")}
                    </button>
                  )}
                </div>
                {hooksStatus === "error" && (
                  <p className={styles.hooks_error}>{t("hooks.install_error", { error: hooksError })}</p>
                )}

                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.sources")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.sources_desc")}
                  </span>
                </div>
                {sources.map((source) => (
                  <div className={styles.row} key={source.name}>
                    <div className={styles.source_row}>
                      <AgentSourceIcon source={source.name} />
                      <span className={styles.row_label}>
                        {t(`settings.source_name.${source.name}`)}
                      </span>
                      {!source.available && (
                        <span className={styles.source_unavailable}>
                          {t("settings.source_not_detected")}
                        </span>
                      )}
                    </div>
                    <label className={styles.toggle}>
                      <input
                        type="checkbox"
                        checked={source.enabled}
                        onChange={(e) => handleToggleSource(source.name, e.target.checked)}
                      />
                      <span className={styles.toggle_slider} />
                    </label>
                  </div>
                ))}
                {sourcesNeedRestart && (
                  <div className={styles.sources_restart_row}>
                    <span className={styles.sources_restart_hint}>{t("settings.sources_restart")}</span>
                    <button
                      className={styles.sources_restart_btn}
                      onClick={() => invoke("restart_app")}
                    >
                      {t("settings.sources_restart_btn")}
                    </button>
                  </div>
                )}
              </div>
            )}

            {/* ── Mobile Access ── */}
            {activeTab === "mobile" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.mobile_access")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.mobile_desc")}
                  </span>
                </div>

                <div className={styles.row}>
                  {mobileAccess?.running ? (
                    <button className={styles.switch_btn} onClick={handleDisableMobileAccess}>
                      {t("settings.mobile_disable")}
                    </button>
                  ) : (
                    <button
                      className={styles.hooks_install_btn}
                      onClick={handleEnableMobileAccess}
                      disabled={mobileLoading}
                    >
                      {mobileLoading ? t("account.loading") : t("settings.mobile_enable")}
                    </button>
                  )}
                </div>

                {mobileAccess?.running && (
                  <>
                    {/* Status info */}
                    <div className={styles.row}>
                      <span className={styles.row_label}>{t("settings.mobile_status")}</span>
                      <span className={styles.hooks_ok}>
                        {mobileAccess.tunnelUrl ? t("settings.mobile_tunnel_active") : t("settings.mobile_local_only")}
                      </span>
                    </div>

                    {mobileAccess.tunnelUrl && (
                      <div className={styles.row}>
                        <span className={styles.row_label}>URL</span>
                        <span className={styles.connection_badge} style={{ fontSize: 10, wordBreak: "break-all" }}>
                          {mobileAccess.tunnelUrl}
                        </span>
                      </div>
                    )}

                    <div className={styles.row}>
                      <span className={styles.row_label}>{t("settings.mobile_clients")}</span>
                      <span>{mobileAccess.connectedClients}</span>
                    </div>

                    {/* QR Code */}
                    {mobileQrData && (
                      <div className={styles.row} style={{ justifyContent: "center", padding: "16px 0" }}>
                        <div style={{
                          background: "#fff",
                          padding: 16,
                          borderRadius: 8,
                          display: "inline-block",
                        }}>
                          <QRCodeCanvas value={mobileQrData} size={200} />
                        </div>
                      </div>
                    )}

                    {!mobileAccess.tunnelUrl && !mobileAccess.cloudflaredAvailable && (
                      <div className={styles.row}>
                        <span className={styles.hooks_warn}>
                          {t("settings.mobile_no_cloudflared")}
                        </span>
                      </div>
                    )}
                  </>
                )}
              </div>
            )}

            {/* ── Notifications ── */}
            {activeTab === "notifications" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.notification_mode")}</div>
                {(["all", "user_action", "none"] as const).map((mode) => (
                  <label className={styles.radio_row} key={mode}>
                    <input
                      type="radio"
                      name="notif-mode"
                      checked={notifMode === mode}
                      onChange={() => handleNotifModeChange(mode)}
                      className={styles.radio_input}
                    />
                    <div className={styles.radio_label}>
                      <span className={styles.radio_title}>
                        {t(`settings.notify_${mode}`)}
                      </span>
                      <span className={styles.radio_desc}>
                        {t(`settings.notify_${mode}_desc`)}
                      </span>
                    </div>
                  </label>
                ))}

                <div className={styles.section_title} style={{ marginTop: 18 }}>
                  {t("settings.notification_permission")}
                </div>
                <div className={styles.row}>
                  {notifPermission === true && (
                    <span className={styles.hooks_ok}>
                      {t("settings.notification_granted")}
                    </span>
                  )}
                  {notifPermission === false && (
                    <div className={styles.notif_denied_row}>
                      <span className={styles.notif_denied_text}>
                        {t("settings.notification_denied")}
                      </span>
                      <button
                        className={styles.hooks_install_btn}
                        onClick={handleRequestPermission}
                      >
                        {t("settings.notification_open_settings")}
                      </button>
                    </div>
                  )}
                  {notifPermission === null && (
                    <span className={styles.row_label} style={{ color: "var(--color-text-dim)" }}>
                      {t("account.loading")}
                    </span>
                  )}
                </div>
              </div>
            )}

            {/* ── Sound ── */}
            {activeTab === "sound" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.tts")}</div>
                {(["chime_and_speech", "chime_only", "off"] as const).map((mode) => (
                  <label className={styles.radio_row} key={mode}>
                    <input
                      type="radio"
                      name="tts-mode"
                      checked={ttsMode === mode}
                      onChange={() => handleTtsModeChange(mode)}
                      className={styles.radio_input}
                    />
                    <div className={styles.radio_label}>
                      <span className={styles.radio_title}>
                        {t(`settings.tts_${mode}`)}
                      </span>
                      <span className={styles.radio_desc}>
                        {t(`settings.tts_${mode}_desc`)}
                      </span>
                    </div>
                  </label>
                ))}

                {ttsMode !== "off" && (
                  <>
                    <div className={styles.section_title} style={{ marginTop: 18 }}>
                      {t("settings.chime_sound")}
                    </div>
                    <div className={styles.row}>
                      <select
                        className={styles.select}
                        value={chimePreset}
                        onChange={(e) => handleChimeChange(e.target.value as ChimePreset)}
                      >
                        {CHIME_PRESETS.map((p) => (
                          <option key={p} value={p}>{t(`settings.chime_${p}`)}</option>
                        ))}
                      </select>
                      <button
                        className={styles.preview_btn}
                        onClick={() => playChime(chimePreset)}
                      >
                        {t("settings.preview")}
                      </button>
                    </div>
                  </>
                )}

                {ttsMode === "chime_and_speech" && voices.length > 0 && (
                  <>
                    <div className={styles.section_title} style={{ marginTop: 18 }}>
                      {t("settings.tts_voice")}
                    </div>
                    <div className={styles.row}>
                      <select
                        className={styles.select}
                        value={ttsVoice}
                        onChange={(e) => handleVoiceChange(e.target.value)}
                      >
                        <option value="">{t("settings.tts_voice_default")}</option>
                        {voices.map((v) => (
                          <option key={v.name} value={v.name}>
                            {v.display_name} ({v.gender}, {v.lang})
                          </option>
                        ))}
                      </select>
                      <button
                        className={styles.preview_btn}
                        onClick={() => speakText(t("settings.tts_preview_text"), ttsVoice || undefined)}
                      >
                        {t("settings.preview")}
                      </button>
                    </div>
                  </>
                )}
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
