import { invoke } from "@tauri-apps/api/core";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useSessionsStore, useOverlayStore } from "../store";
import { getItem, setItem } from "../storage";
import { type ChimePreset, CHIME_PRESETS, playChime } from "../audio";
import { ThemeToggle } from "./ThemeToggle";
import { LanguageSwitcher } from "./LanguageSwitcher";
import styles from "./Onboarding.module.css";

// ── Types ────────────────────────────────────────────────────────────────────

interface DetectedTools {
  cli: boolean;
  vscode: boolean;
  jetbrains: boolean;
  desktop: boolean;
  cursor: boolean;
  openclaw: boolean;
  codex: boolean;
}

interface SetupStatus {
  cli_installed: boolean;
  cli_path: string | null;
  claude_dir_exists: boolean;
  detected_tools: DetectedTools;
  logged_in: boolean;
  has_sessions: boolean;
  credentials_valid: boolean | null;
}

type ToolTab = "cli" | "vscode" | "jetbrains" | "desktop";

type Issue =
  | "no_claude_at_all"
  | "not_logged_in"
  | "credentials_invalid";

interface HookSetupPlan {
  toAdd: string[];
  hooksGloballyDisabled: boolean;
  alreadyInstalled: boolean;
}

type NotificationMode = "all" | "user_action" | "none";

// ── Confetti ─────────────────────────────────────────────────────────────────

const CONFETTI_COUNT = 60;
const CONFETTI_COLORS = [
  "#d97757", "#f0a070", "#4ade80", "#60a5fa", "#c084fc",
  "#fbbf24", "#22d3ee", "#f87171", "#a3e635", "#fb923c",
];

interface ConfettiPiece {
  id: number;
  x: number;      // start x %
  delay: number;   // animation delay s
  duration: number; // fall duration s
  color: string;
  size: number;
  rotation: number;
  drift: number;   // horizontal drift px
}

function generateConfetti(): ConfettiPiece[] {
  return Array.from({ length: CONFETTI_COUNT }, (_, i) => ({
    id: i,
    x: Math.random() * 100,
    delay: Math.random() * 0.8,
    duration: 1.5 + Math.random() * 2,
    color: CONFETTI_COLORS[Math.floor(Math.random() * CONFETTI_COLORS.length)],
    size: 4 + Math.random() * 6,
    rotation: Math.random() * 360,
    drift: (Math.random() - 0.5) * 120,
  }));
}

function Confetti() {
  const [pieces] = useState(generateConfetti);
  return (
    <div className={styles.confetti_container} aria-hidden>
      {pieces.map((p) => (
        <div
          key={p.id}
          className={styles.confetti_piece}
          style={{
            left: `${p.x}%`,
            animationDelay: `${p.delay}s`,
            animationDuration: `${p.duration}s`,
            backgroundColor: p.color,
            width: p.size,
            height: p.size * 1.4,
            transform: `rotate(${p.rotation}deg)`,
            ["--drift" as string]: `${p.drift}px`,
          }}
        />
      ))}
    </div>
  );
}

// ── Celebration sound ────────────────────────────────────────────────────────

function playCelebrationSound() {
  try {
    const ctx = new AudioContext();
    const now = ctx.currentTime;

    // A bright ascending arpeggio: C5 → E5 → G5 → C6
    const notes = [523.25, 659.25, 783.99, 1046.50];
    notes.forEach((freq, i) => {
      const osc = ctx.createOscillator();
      const gain = ctx.createGain();
      osc.type = "sine";
      osc.frequency.value = freq;
      gain.gain.setValueAtTime(0.15, now + i * 0.12);
      gain.gain.exponentialRampToValueAtTime(0.001, now + i * 0.12 + 0.4);
      osc.connect(gain).connect(ctx.destination);
      osc.start(now + i * 0.12);
      osc.stop(now + i * 0.12 + 0.4);
    });

    // Clean up
    setTimeout(() => ctx.close(), 2000);
  } catch {
    // Audio not available — silently ignore
  }
}

// ── Shared sub-components ────────────────────────────────────────────────────

function CopyableCommand({ cmd }: { cmd: string }) {
  const { t } = useTranslation();
  const [copied, setCopied] = useState(false);

  const copy = () => {
    writeText(cmd);
    setCopied(true);
    setTimeout(() => setCopied(false), 2000);
  };

  return (
    <div className={styles.code_block} onClick={copy} title={t("onboarding.copy_hint")}>
      <span className={styles.code_text}>{cmd}</span>
      {copied ? (
        <span className={styles.copied}>Copied!</span>
      ) : (
        <span className={styles.copy_icon}>&#x2398;</span>
      )}
    </div>
  );
}

// ── Tool tabs for login guidance ─────────────────────────────────────────────

const ALL_TABS: ToolTab[] = ["cli", "vscode", "jetbrains", "desktop"];

function getRecommendedTab(tools: DetectedTools): ToolTab {
  // Prefer the tool the user actually has installed
  if (tools.vscode) return "vscode";
  if (tools.cli) return "cli";
  if (tools.desktop) return "desktop";
  if (tools.jetbrains) return "jetbrains";
  return "cli"; // default
}

function LoginTabContent({ tab }: { tab: ToolTab }) {
  const { t } = useTranslation();

  switch (tab) {
    case "cli":
      return (
        <div className={styles.tab_content}>
          <p className={styles.hint}>{t("onboarding.login_tabs.cli.hint")}</p>
          <CopyableCommand cmd="claude login" />
          <p className={styles.hint}>{t("onboarding.login_tabs.cli.browser")}</p>
        </div>
      );
    case "vscode":
      return (
        <div className={styles.tab_content}>
          <p className={styles.hint}>{t("onboarding.login_tabs.vscode.step1")}</p>
          <p className={styles.hint}>{t("onboarding.login_tabs.vscode.step2")}</p>
          <p className={styles.hint}>{t("onboarding.login_tabs.vscode.step3")}</p>
        </div>
      );
    case "jetbrains":
      return (
        <div className={styles.tab_content}>
          <p className={styles.hint}>{t("onboarding.login_tabs.jetbrains.step1")}</p>
          <p className={styles.hint}>{t("onboarding.login_tabs.jetbrains.step2")}</p>
        </div>
      );
    case "desktop":
      return (
        <div className={styles.tab_content}>
          <p className={styles.hint}>{t("onboarding.login_tabs.desktop.step1")}</p>
          <p className={styles.hint}>{t("onboarding.login_tabs.desktop.step2")}</p>
        </div>
      );
  }
}

function NotLoggedInCard({ tools }: { tools: DetectedTools }) {
  const { t } = useTranslation();
  const [activeTab, setActiveTab] = useState<ToolTab>(() => getRecommendedTab(tools));

  return (
    <div className={`${styles.card} ${styles.card_warn}`}>
      <div className={styles.card_header}>
        <span className={styles.card_icon}>&#x1F511;</span>
        <span className={styles.card_title}>{t("onboarding.not_logged_in.title")}</span>
      </div>
      <p className={styles.card_description}>{t("onboarding.not_logged_in.description")}</p>

      <div className={styles.tabs}>
        {ALL_TABS.map((tab) => (
          <button
            key={tab}
            className={`${styles.tab} ${activeTab === tab ? styles.tab_active : ""} ${
              tools[tab] ? styles.tab_detected : ""
            }`}
            onClick={() => setActiveTab(tab)}
          >
            {t(`onboarding.login_tabs.${tab}.label`)}
            {tools[tab] && <span className={styles.tab_badge}>{t("onboarding.login_tabs.detected")}</span>}
          </button>
        ))}
      </div>

      <LoginTabContent tab={activeTab} />

      <div className={styles.divider}>{t("onboarding.not_logged_in.api_key")}</div>
      <p className={styles.hint}>{t("onboarding.not_logged_in.api_key_hint")}</p>
    </div>
  );
}

// ── Other issue cards ────────────────────────────────────────────────────────

function NoClaudeAtAllCard() {
  const { t } = useTranslation();
  return (
    <div className={`${styles.card} ${styles.card_error}`}>
      <div className={styles.card_header}>
        <span className={styles.card_icon}>&#x26A0;</span>
        <span className={styles.card_title}>{t("onboarding.no_claude_at_all.title")}</span>
      </div>
      <p className={styles.card_description}>{t("onboarding.no_claude_at_all.description")}</p>
      <div className={styles.solutions}>
        <div className={styles.solution}>
          <span className={styles.solution_label}>{t("onboarding.cli_not_installed.install_npm")}</span>
          <CopyableCommand cmd={t("onboarding.cli_not_installed.install_npm_cmd")} />
        </div>
        <div className={styles.solution}>
          <span className={styles.solution_label}>{t("onboarding.cli_not_installed.install_brew")}</span>
          <CopyableCommand cmd={t("onboarding.cli_not_installed.install_brew_cmd")} />
        </div>
        <div className={styles.solution}>
          <span className={styles.solution_label}>{t("onboarding.no_claude_at_all.install_vscode")}</span>
          <p className={styles.hint}>{t("onboarding.no_claude_at_all.install_vscode_hint")}</p>
        </div>
        <p className={styles.hint}>{t("onboarding.cli_not_installed.after_install")}</p>
      </div>
    </div>
  );
}

function CredentialsInvalidCard() {
  const { t } = useTranslation();
  return (
    <div className={`${styles.card} ${styles.card_warn}`}>
      <div className={styles.card_header}>
        <span className={styles.card_icon}>&#x1F504;</span>
        <span className={styles.card_title}>{t("onboarding.credentials_invalid.title")}</span>
      </div>
      <p className={styles.card_description}>{t("onboarding.credentials_invalid.description")}</p>
      <div className={styles.solutions}>
        <div className={styles.solution}>
          <CopyableCommand cmd={t("onboarding.credentials_invalid.relogin_cmd")} />
          <p className={styles.hint}>{t("onboarding.credentials_invalid.relogin_hint")}</p>
        </div>
        <p className={styles.hint}>{t("onboarding.credentials_invalid.check_network")}</p>
        <p className={styles.hint}>{t("onboarding.credentials_invalid.still_works")}</p>
      </div>
    </div>
  );
}

// ── Waiting for first session ────────────────────────────────────────────────

function WaitingForSession() {
  const { t } = useTranslation();
  return (
    <div className={styles.waiting}>
      <div className={styles.pulse_ring} />
      <div className={styles.waiting_icon}>&#x1F50D;</div>
      <h3 className={styles.waiting_title}>{t("onboarding.waiting.title")}</h3>
      <p className={styles.waiting_description}>{t("onboarding.waiting.description")}</p>
      <div className={styles.waiting_hints}>
        <p className={styles.hint}>{t("onboarding.waiting.hint_terminal")}</p>
        <p className={styles.hint}>{t("onboarding.waiting.hint_ide")}</p>
      </div>
    </div>
  );
}

function CelebrationView({ onDismiss }: { onDismiss: () => void }) {
  const { t } = useTranslation();
  const soundPlayed = useRef(false);

  useEffect(() => {
    if (!soundPlayed.current) {
      soundPlayed.current = true;
      playCelebrationSound();
    }
  }, []);

  return (
    <>
      <Confetti />
      <div className={styles.celebration}>
        <div className={styles.celebration_icon}>&#x1F389;</div>
        <h2 className={styles.celebration_title}>{t("onboarding.celebration.title")}</h2>
        <p className={styles.celebration_description}>{t("onboarding.celebration.description")}</p>
        <button className={styles.btn_primary} onClick={onDismiss}>
          {t("onboarding.dismiss")}
        </button>
      </div>
    </>
  );
}

// ── Feature highlights card ──────────────────────────────────────────────

interface FeatureItem {
  icon: string;
  titleKey: string;
  descKey: string;
}

const FEATURES: FeatureItem[] = [
  { icon: "\u{1F4CA}", titleKey: "onboarding.features.ai_summary_title", descKey: "onboarding.features.ai_summary_desc" },
  { icon: "\u{1F9E0}", titleKey: "onboarding.features.lessons_title", descKey: "onboarding.features.lessons_desc" },
  { icon: "\u{1F6A6}", titleKey: "onboarding.features.live_status_title", descKey: "onboarding.features.live_status_desc" },
  { icon: "\u{1F6E1}", titleKey: "onboarding.features.audit_title", descKey: "onboarding.features.audit_desc" },
  { icon: "\u{1F4DD}", titleKey: "onboarding.features.memory_title", descKey: "onboarding.features.memory_desc" },
];

function FeaturesCard() {
  const { t } = useTranslation();

  return (
    <div className={`${styles.card} ${styles.card_info}`}>
      <div className={styles.card_header}>
        <span className={styles.card_icon}>&#x2728;</span>
        <span className={styles.card_title}>{t("onboarding.features.title")}</span>
      </div>
      <p className={styles.card_description}>{t("onboarding.features.description")}</p>
      <div className={styles.feature_list}>
        {FEATURES.map((f, i) => (
          <div key={i} className={styles.feature_item}>
            <span className={styles.feature_icon}>{f.icon}</span>
            <div className={styles.feature_text}>
              <span className={styles.feature_title}>{t(f.titleKey)}</span>
              <span className={styles.hint}>{t(f.descKey)}</span>
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}

// ── Source selection card ─────────────────────────────────────────────────────

interface SourceInfo {
  name: string;
  enabled: boolean;
  available: boolean;
}

function SourceSelectionCard({
  sources,
  onToggle,
}: {
  sources: SourceInfo[];
  onToggle: (name: string, enabled: boolean) => void;
}) {
  const { t } = useTranslation();

  return (
    <div className={`${styles.card} ${styles.card_info}`}>
      <div className={styles.card_header}>
        <span className={styles.card_icon}>&#x1F50C;</span>
        <span className={styles.card_title}>{t("onboarding.sources.title")}</span>
      </div>
      <p className={styles.card_description}>{t("onboarding.sources.description")}</p>
      <div className={styles.source_list}>
        {sources.map((source) => (
          <label key={source.name} className={styles.source_item}>
            <input
              type="checkbox"
              checked={source.enabled}
              onChange={(e) => onToggle(source.name, e.target.checked)}
              className={styles.source_checkbox}
            />
            <span className={styles.source_name}>
              {t(`settings.source_name.${source.name}`)}
            </span>
            {source.available ? (
              <span className={styles.source_detected}>{t("onboarding.sources.detected")}</span>
            ) : (
              <span className={styles.source_not_found}>{t("onboarding.sources.not_detected")}</span>
            )}
          </label>
        ))}
      </div>
    </div>
  );
}

// ── Appearance card (theme + language) ───────────────────────────────────

function AppearanceCard() {
  const { t } = useTranslation();

  return (
    <div className={`${styles.card} ${styles.card_info}`}>
      <div className={styles.card_header}>
        <span className={styles.card_icon}>&#x1F3A8;</span>
        <span className={styles.card_title}>{t("onboarding.settings_appearance.title")}</span>
      </div>
      <p className={styles.card_description}>{t("onboarding.settings_appearance.description")}</p>

      <div className={styles.settings_group}>
        <span className={styles.settings_label}>{t("settings.theme")}</span>
        <ThemeToggle />
      </div>

      <div className={styles.settings_group}>
        <span className={styles.settings_label}>{t("settings.language")}</span>
        <LanguageSwitcher />
      </div>
    </div>
  );
}

// ── Notification & mascot settings card ──────────────────────────────────

type TtsMode = "chime_and_speech" | "chime_only" | "off";

function NotificationSettingsCard({
  notifMode,
  onNotifModeChange,
  ttsMode,
  onTtsModeChange,
  chimePreset,
  onChimeChange,
  personalizedMascot,
  onTogglePersonalizedMascot,
  overlayEnabled,
  onToggleOverlay,
  userTitle,
  onUserTitleChange,
}: {
  notifMode: NotificationMode;
  onNotifModeChange: (mode: NotificationMode) => void;
  ttsMode: TtsMode;
  onTtsModeChange: (mode: TtsMode) => void;
  chimePreset: ChimePreset;
  onChimeChange: (preset: ChimePreset) => void;
  personalizedMascot: boolean;
  onTogglePersonalizedMascot: (enabled: boolean) => void;
  overlayEnabled: boolean;
  onToggleOverlay: (enabled: boolean) => void;
  userTitle: string;
  onUserTitleChange: (title: string) => void;
}) {
  const { t } = useTranslation();

  return (
    <div className={`${styles.card} ${styles.card_info}`}>
      <div className={styles.card_header}>
        <span className={styles.card_icon}>&#x1F514;</span>
        <span className={styles.card_title}>{t("onboarding.settings_notif.title")}</span>
      </div>
      <p className={styles.card_description}>{t("onboarding.settings_notif.description")}</p>

      {/* Notification mode */}
      <div className={styles.settings_group}>
        <span className={styles.settings_label}>{t("settings.notification_mode")}</span>
        {(["all", "user_action", "none"] as const).map((mode) => (
          <label className={styles.radio_item} key={mode}>
            <input
              type="radio"
              name="onboard-notif-mode"
              checked={notifMode === mode}
              onChange={() => onNotifModeChange(mode)}
            />
            <div>
              <span className={styles.radio_title}>{t(`settings.notify_${mode}`)}</span>
              <span className={styles.hint}>{t(`settings.notify_${mode}_desc`)}</span>
            </div>
          </label>
        ))}
      </div>

      {/* Alert sound */}
      <div className={styles.settings_group}>
        <span className={styles.settings_label}>{t("onboarding.settings_notif.sound_title")}</span>
        {(["chime_and_speech", "chime_only", "off"] as const).map((mode) => (
          <label className={styles.radio_item} key={mode}>
            <input
              type="radio"
              name="onboard-tts-mode"
              checked={ttsMode === mode}
              onChange={() => onTtsModeChange(mode)}
            />
            <div>
              <span className={styles.radio_title}>{t(`settings.tts_${mode}`)}</span>
              <span className={styles.hint}>{t(`settings.tts_${mode}_desc`)}</span>
            </div>
          </label>
        ))}
        {ttsMode !== "off" && (
          <div className={styles.chime_row}>
            <span className={styles.hint}>{t("settings.chime_sound")}</span>
            <select
              className={styles.chime_select}
              value={chimePreset}
              onChange={(e) => onChimeChange(e.target.value as ChimePreset)}
            >
              {CHIME_PRESETS.map((p) => (
                <option key={p} value={p}>{t(`settings.chime_${p}`)}</option>
              ))}
            </select>
            <button
              className={styles.preview_btn}
              onClick={() => playChime(chimePreset)}
              type="button"
            >
              &#x25B6;
            </button>
          </div>
        )}
      </div>

      {/* User title */}
      <div className={styles.settings_group}>
        <span className={styles.settings_label}>{t("settings.user_title")}</span>
        <span className={styles.hint}>{t("settings.user_title_desc")}</span>
        <input
          type="text"
          value={userTitle}
          placeholder={t("settings.user_title_placeholder")}
          onChange={(e) => onUserTitleChange(e.target.value)}
          style={{
            padding: "6px 10px",
            borderRadius: 6,
            border: "1px solid var(--color-border)",
            background: "var(--color-bg-input, var(--color-bg))",
            color: "var(--color-text)",
            fontSize: 13,
            width: 160,
          }}
        />
      </div>

      {/* Mascot & overlay */}
      <div className={styles.settings_group}>
        <span className={styles.settings_label}>{t("onboarding.settings_notif.mascot_title")}</span>
        <label className={styles.toggle_item}>
          <span>{t("settings.personalized_mascot")}</span>
          <input
            type="checkbox"
            checked={personalizedMascot}
            onChange={(e) => onTogglePersonalizedMascot(e.target.checked)}
            className={styles.source_checkbox}
          />
        </label>
        <span className={styles.hint}>{t("settings.personalized_mascot_desc")}</span>
        <label className={styles.toggle_item}>
          <span>{t("settings.overlay")}</span>
          <input
            type="checkbox"
            checked={overlayEnabled}
            onChange={(e) => onToggleOverlay(e.target.checked)}
            className={styles.source_checkbox}
          />
        </label>
        <span className={styles.hint}>{t("settings.overlay_desc")}</span>
      </div>
    </div>
  );
}

// ── Hooks setup card ────────────────────────────────────────────────────

function HooksSetupCard({
  hooksPlan,
  onInstall,
  status,
  errorMsg,
}: {
  hooksPlan: HookSetupPlan;
  onInstall: () => void;
  status: "idle" | "installing" | "success" | "error";
  errorMsg: string;
}) {
  const { t } = useTranslation();

  if (hooksPlan.alreadyInstalled || status === "success") {
    return (
      <div className={`${styles.card} ${styles.card_ok}`}>
        <div className={styles.card_header}>
          <span className={styles.card_icon}>&#x2705;</span>
          <span className={styles.card_title}>{t("hooks.installed")}</span>
        </div>
      </div>
    );
  }

  return (
    <div className={`${styles.card} ${styles.card_warn}`}>
      <div className={styles.card_header}>
        <span className={styles.card_icon}>&#x1FA9D;</span>
        <span className={styles.card_title}>{t("onboarding.hooks_setup.title")}</span>
      </div>
      <p className={styles.card_description}>{t("onboarding.hooks_setup.description")}</p>
      <div className={styles.hooks_actions}>
        <button
          className={styles.btn_primary}
          onClick={onInstall}
          disabled={status === "installing"}
          style={{ padding: "8px 20px", fontSize: 12 }}
        >
          {status === "installing" ? t("account.loading") : t("hooks.install")}
        </button>
      </div>
      {hooksPlan.hooksGloballyDisabled && (
        <p className={styles.hint}>{t("onboarding.hooks_setup.disabled_warning")}</p>
      )}
      {status === "error" && (
        <p className={styles.hint} style={{ color: "var(--color-error-fg)" }}>
          {t("hooks.install_error", { error: errorMsg })}
        </p>
      )}
    </div>
  );
}

// ── Main Onboarding component ────────────────────────────────────────────────

export function Onboarding({ onDismiss }: { onDismiss: () => void }) {
  const { t } = useTranslation();
  const sessions = useSessionsStore((s) => s.sessions);
  const [status, setStatus] = useState<SetupStatus | null>(null);
  const [loading, setLoading] = useState(true);
  const [accountError, setAccountError] = useState(false);
  const [celebrating, setCelebrating] = useState(false);
  const prevSessionCount = useRef(0);

  // ── Sources config ───────────────────────────────────────────────────────
  const [sources, setSources] = useState<SourceInfo[]>([]);
  const sourcesChanged = useRef(false);

  useEffect(() => {
    invoke<SourceInfo[]>("get_sources_config").then(setSources).catch(() => {});
  }, []);

  const handleToggleSource = useCallback(async (name: string, enabled: boolean) => {
    try {
      await invoke("set_source_enabled", { name, enabled });
      setSources((prev) => prev.map((s) => (s.name === name ? { ...s, enabled } : s)));
      sourcesChanged.current = true;
    } catch {
      // ignore
    }
  }, []);

  // ── Hooks state ────────────────────────────────────────────────────────
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

  // ── Notification state ──────────────────────────────────────────────────
  const [notifMode, setNotifMode] = useState<NotificationMode>(
    () => (getItem("notification-mode") as NotificationMode) || "user_action",
  );

  const handleNotifModeChange = useCallback((mode: NotificationMode) => {
    setNotifMode(mode);
    setItem("notification-mode", mode);
    invoke("set_notification_mode", { mode }).catch(() => {});
  }, []);

  // Request notification permission on mount if not granted
  useEffect(() => {
    isPermissionGranted().then((granted) => {
      if (!granted) requestPermission().catch(() => {});
    }).catch(() => {});
  }, []);

  // ── TTS / chime state ──────────────────────────────────────────────────
  const [ttsMode, setTtsMode] = useState<TtsMode>(
    () => (getItem("tts-mode") as TtsMode) || "off",
  );

  const handleTtsModeChange = useCallback((mode: TtsMode) => {
    setTtsMode(mode);
    setItem("tts-mode", mode);
  }, []);

  const [chimePreset, setChimePreset] = useState<ChimePreset>(
    () => (getItem("chime-sound") as ChimePreset) || "ding_dong",
  );

  const handleChimeChange = useCallback((preset: ChimePreset) => {
    setChimePreset(preset);
    setItem("chime-sound", preset);
    playChime(preset);
  }, []);

  // ── User title state ────────────────────────────────────────────────────
  const [userTitle, setUserTitle] = useState(() => getItem("user-title") || "");

  const handleUserTitleChange = useCallback((title: string) => {
    setUserTitle(title);
    setItem("user-title", title);
    invoke("set_user_title", { title }).catch(() => {});
  }, []);

  // ── Mascot & overlay state ──────────────────────────────────────────────
  const [personalizedMascot, setPersonalizedMascot] = useState(
    () => getItem("personalized-mascot") !== "false",
  );
  const overlayEnabled = useOverlayStore((s) => s.enabled);
  const setOverlayEnabled = useOverlayStore((s) => s.setEnabled);

  const handleTogglePersonalizedMascot = useCallback((enabled: boolean) => {
    setPersonalizedMascot(enabled);
    setItem("personalized-mascot", enabled ? "true" : "false");
  }, []);

  // Show source selection only when there are multiple source types detected
  const hasMultipleSources = useMemo(() => {
    return sources.filter((s) => s.available).length > 1;
  }, [sources]);

  // Wrap onDismiss: if sources were changed, restart to pick up new config
  const handleDismiss = useCallback(() => {
    if (sourcesChanged.current) {
      invoke("restart_app");
    } else {
      onDismiss();
    }
  }, [onDismiss]);

  const check = useCallback(async () => {
    setLoading(true);
    try {
      const s = await invoke<SetupStatus>("check_setup_status");

      if (s.logged_in) {
        try {
          await invoke("get_account_info");
          s.credentials_valid = true;
          setAccountError(false);
        } catch {
          s.credentials_valid = false;
          setAccountError(true);
        }
      }

      s.has_sessions = s.has_sessions || sessions.length > 0;
      setStatus(s);
    } catch {
      setStatus({
        cli_installed: false,
        cli_path: null,
        claude_dir_exists: false,
        detected_tools: { cli: false, vscode: false, jetbrains: false, desktop: false, cursor: false, openclaw: false, codex: false },
        logged_in: false,
        has_sessions: false,
        credentials_valid: null,
      });
    } finally {
      setLoading(false);
    }
  }, [sessions.length]);

  useEffect(() => {
    check();
  }, []);

  // Watch for first session appearing → trigger celebration.
  // Only celebrate if the backend also confirmed there were no sessions before
  // (avoids false celebration for existing users whose sessions loaded after the check).
  useEffect(() => {
    if (
      !loading &&
      !celebrating &&
      sessions.length > 0 &&
      prevSessionCount.current === 0 &&
      status !== null &&
      !status.has_sessions
    ) {
      setCelebrating(true);
    }
    prevSessionCount.current = sessions.length;
  }, [sessions.length, loading, celebrating, status]);

  // Determine issues
  const issues: Issue[] = [];
  if (status) {
    const hasAnyClaude =
      status.cli_installed || status.claude_dir_exists || status.logged_in || status.has_sessions;
    const hasCursor = status.detected_tools.cursor;

    if (!hasAnyClaude && !hasCursor) {
      // Neither Claude Code nor Cursor detected
      issues.push("no_claude_at_all");
    } else if (!hasAnyClaude && hasCursor) {
      // Cursor-only user — Claude Code login issues are not blockers.
      // Skip login checks; they can use Fleet with Cursor sessions only.
    } else {
      if (!status.logged_in) issues.push("not_logged_in");
      if (status.logged_in && accountError) issues.push("credentials_invalid");
    }
  }

  const noIssues = status && issues.length === 0;
  const showWaiting = noIssues && !status?.has_sessions && !celebrating;

  // Show hooks setup when Claude Code is among the detected tools and hooks are not installed
  const hasClaudeCode = status && (
    status.cli_installed || status.detected_tools.cli || status.detected_tools.vscode || status.detected_tools.jetbrains
  );
  const showHooksSetup = hasClaudeCode && hooksPlan && !hooksPlan.alreadyInstalled && hooksStatus !== "success";

  return (
    <div className={styles.overlay}>
      <div className={styles.container}>
        {celebrating ? (
          <CelebrationView onDismiss={handleDismiss} />
        ) : (
          <>
            <div className={styles.header}>
              <img src="/app-icon.png" className={styles.logo} alt="Claw Fleet" />
              <h1 className={styles.title}>{t("onboarding.welcome")}</h1>
              <p className={styles.subtitle}>{t("onboarding.subtitle")}</p>
            </div>

            {loading ? (
              <div className={styles.loading}>
                <div className={styles.spinner} />
                <span className={styles.loading_text}>{t("onboarding.checking")}</span>
              </div>
            ) : (
              <>
                {issues.length > 0 && (
                  <div className={styles.cards}>
                    {issues.includes("no_claude_at_all") && <NoClaudeAtAllCard />}
                    {issues.includes("not_logged_in") && status && (
                      <NotLoggedInCard tools={status.detected_tools} />
                    )}
                    {issues.includes("credentials_invalid") && <CredentialsInvalidCard />}
                  </div>
                )}

                <div className={styles.cards}>
                  <FeaturesCard />
                </div>

                <div className={styles.cards}>
                  <AppearanceCard />
                </div>

                {hasMultipleSources && sources.length > 0 && (
                  <div className={styles.cards}>
                    <SourceSelectionCard sources={sources} onToggle={handleToggleSource} />
                  </div>
                )}

                {noIssues && (
                  <div className={styles.cards}>
                    <NotificationSettingsCard
                      notifMode={notifMode}
                      onNotifModeChange={handleNotifModeChange}
                      ttsMode={ttsMode}
                      onTtsModeChange={handleTtsModeChange}
                      chimePreset={chimePreset}
                      onChimeChange={handleChimeChange}
                      personalizedMascot={personalizedMascot}
                      onTogglePersonalizedMascot={handleTogglePersonalizedMascot}
                      overlayEnabled={overlayEnabled}
                      onToggleOverlay={setOverlayEnabled}
                      userTitle={userTitle}
                      onUserTitleChange={handleUserTitleChange}
                    />
                    {showHooksSetup && hooksPlan && (
                      <HooksSetupCard
                        hooksPlan={hooksPlan}
                        onInstall={handleInstallHooks}
                        status={hooksStatus}
                        errorMsg={hooksError}
                      />
                    )}
                  </div>
                )}

                {showWaiting && <WaitingForSession />}

                {noIssues && status?.has_sessions && !celebrating && (
                  <div className={styles.cards}>
                    <div className={`${styles.card} ${styles.card_ok}`}>
                      <div className={styles.card_header}>
                        <span className={styles.card_icon}>&#x2705;</span>
                        <span className={styles.card_title}>{t("onboarding.all_good.title")}</span>
                      </div>
                      <p className={styles.card_description}>{t("onboarding.all_good.description")}</p>
                    </div>
                  </div>
                )}
              </>
            )}

            <div className={styles.footer}>
              <button className={styles.btn_secondary} onClick={handleDismiss}>
                {t("onboarding.skip")}
              </button>
              {!loading && issues.length > 0 && (
                <button className={styles.btn_secondary} onClick={check}>
                  {t("onboarding.recheck")}
                </button>
              )}
              {noIssues && status?.has_sessions && (
                <button className={styles.btn_primary} onClick={handleDismiss}>
                  {t("onboarding.dismiss")}
                </button>
              )}
            </div>
          </>
        )}
      </div>
    </div>
  );
}
