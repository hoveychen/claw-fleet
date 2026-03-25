import { invoke } from "@tauri-apps/api/core";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useConnectionStore, useDetailStore } from "../store";
import { AccountInfo } from "./AccountInfo";
import { LanguageSwitcher } from "./LanguageSwitcher";
import { ThemeToggle } from "./ThemeToggle";
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

type SettingsTab = "appearance" | "connection" | "hooks" | "sources" | "account";

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
