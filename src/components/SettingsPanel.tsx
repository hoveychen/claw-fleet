import { invoke } from "@tauri-apps/api/core";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useConnectionStore, useDetailStore, useOverlayStore } from "../store";
import { getItem, setItem } from "../storage";
import { playChime, speakText, getVoices, CHIME_PRESETS, type ChimePreset, type TtsVoice } from "../audio";
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

type NotificationMode = "all" | "user_action" | "none";
type TtsMode = "chime_and_speech" | "chime_only" | "off";
type SettingsTab = "appearance" | "connection" | "hooks" | "sources" | "notifications" | "account";

const tabIcons: Record<SettingsTab, React.ReactNode> = {
  appearance: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="8" cy="8" r="3" />
      <path d="M8 1v2M8 13v2M1 8h2M13 8h2M3.05 3.05l1.41 1.41M11.54 11.54l1.41 1.41M3.05 12.95l1.41-1.41M11.54 4.46l1.41-1.41" />
    </svg>
  ),
  connection: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M6 10l4-4" />
      <path d="M9.5 3.5l1-1a2.12 2.12 0 0 1 3 3l-1 1M6.5 12.5l-1 1a2.12 2.12 0 0 1-3-3l1-1" />
    </svg>
  ),
  hooks: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M5 2v6a3 3 0 0 0 6 0V5" />
      <path d="M8 14v-2" />
      <path d="M5 14h6" />
    </svg>
  ),
  sources: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <ellipse cx="8" cy="4" rx="5" ry="2" />
      <path d="M3 4v4c0 1.1 2.24 2 5 2s5-.9 5-2V4" />
      <path d="M3 8v4c0 1.1 2.24 2 5 2s5-.9 5-2V8" />
    </svg>
  ),
  notifications: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M6 13a2 2 0 0 0 4 0" />
      <path d="M12 7c0-2.76-1.79-5-4-5S4 4.24 4 7c0 3-1.5 4.5-2 5h12c-.5-.5-2-2-2-5z" />
    </svg>
  ),
  account: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="8" cy="5" r="3" />
      <path d="M2.5 14a5.5 5.5 0 0 1 11 0" />
    </svg>
  ),
};

export function SettingsPanel({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation();
  const { connection, disconnect } = useConnectionStore();
  const [activeTab, setActiveTab] = useState<SettingsTab>("appearance");

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

  // ── Overlay state (shared via store) ─────────────────────────────────────
  const overlayEnabled = useOverlayStore((s) => s.enabled);
  const setOverlayEnabled = useOverlayStore((s) => s.setEnabled);

  const handleSwitchConnection = useCallback(async () => {
    await useDetailStore.getState().close();
    await disconnect();
    onClose();
  }, [disconnect, onClose]);

  const hooksInstalled = hooksPlan?.alreadyInstalled || hooksStatus === "success";

  const tabs: { key: SettingsTab; label: string }[] = [
    { key: "appearance", label: t("settings.appearance") },
    { key: "connection", label: t("settings.connection") },
    { key: "hooks", label: t("settings.hooks") },
    { key: "sources", label: t("settings.sources") },
    { key: "notifications", label: t("settings.notifications") },
    { key: "account", label: t("account.panel_title") },
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
            {activeTab === "appearance" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.appearance")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.theme")}</span>
                  <ThemeToggle />
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.language")}</span>
                  <LanguageSwitcher />
                </div>
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
              </div>
            )}

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
              </div>
            )}

            {activeTab === "hooks" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.hooks")}</div>
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
              </div>
            )}

            {activeTab === "sources" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.sources")}</div>
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
                  {t("settings.tts")}
                </div>
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

            {activeTab === "account" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("account.panel_title")}</div>
                <div className={styles.account_embed}>
                  <AccountInfo embedded />
                </div>
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
