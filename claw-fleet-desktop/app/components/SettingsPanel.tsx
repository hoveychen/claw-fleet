import { invoke } from "@tauri-apps/api/core";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useConnectionStore, useDetailStore } from "../store";
import { useKeepAwake } from "../hooks/useKeepAwake";
import { getItem, setItem } from "../storage";
import { playChime, speakText, getVoices, CHIME_PRESETS, type ChimePreset, type TtsVoice } from "../audio";
import { QRCodeCanvas } from "qrcode.react";
import { AccountInfo } from "./AccountInfo";
import { LanguageSwitcher } from "./LanguageSwitcher";
import { ThemeToggle } from "./ThemeToggle";
import { AgentSourceIcon } from "./SessionCard";
import { UsageTrendPanel } from "./UsageTrendPanel";
import styles from "./SettingsPanel.module.css";

interface HookSetupPlan {
  toAdd: string[];
  hooksGloballyDisabled: boolean;
  alreadyInstalled: boolean;
  guardInstalled: boolean;
  elicitationInstalled: boolean;
  interactionModeInstalled: boolean;
  planApprovalInstalled: boolean;
  prdContextInstalled: boolean;
  prdDisciplineInstalled: boolean;
  wikiGuidanceInstalled: boolean;
}

interface SourceInfo {
  name: string;
  enabled: boolean;
  available: boolean;
}

interface ClaudeBinary {
  path: string;
  source: string;
  version: string | null;
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

// Must match `FEISHU_OAUTH_PORT` + callback path in claw-fleet-core/src/feishu.rs.
// Surfaced in the setup checklist so the operator can register it as the
// app's redirect URL (otherwise OAuth fails with error 20029).
const FEISHU_REDIRECT_URL = "http://localhost:51823/feishu/cb";

// Permission scopes to grant under the Feishu console's 权限管理. Superset of
// what's strictly required: the first three are in the OAuth authorize request
// (claw-fleet-core/src/feishu.rs) so a missing one fails consent with error
// 20027; the last two back the bot-send and name/open_id read. Granting extras
// is harmless — under-granting is what breaks the flow. See
// design/feishu-integration.md §2.
const FEISHU_SCOPES: { scope: string; glossKey: string }[] = [
  { scope: "im:message", glossKey: "feishu_scope_message" },
  { scope: "im:message:send_as_bot", glossKey: "feishu_scope_send_as_bot" },
  { scope: "im:message.urgent", glossKey: "feishu_scope_urgent" },
  { scope: "im:resource", glossKey: "feishu_scope_resource" },
  { scope: "contact:user.id:readonly", glossKey: "feishu_scope_contact" },
];
// Redesigned IA: 3 everyday tabs (general / alerts / account) + 4 advanced
// tabs (interaction / model / integration / usage) shown under a collapsible
// "Advanced" group. See the settings-redesign plan.
type SettingsTab = "general" | "alerts" | "account" | "interaction" | "model" | "integration" | "usage";
const BASE_TABS: SettingsTab[] = ["general", "alerts", "account"];
const ADVANCED_TABS: SettingsTab[] = ["interaction", "model", "integration", "usage"];

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
  // Alerts = bell (formerly the Notifications icon); the unified "how Fleet
  // alerts me" tab covers system notifications + chime + speech.
  alerts: (
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
  interaction: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M2 4a1.5 1.5 0 0 1 1.5-1.5h6A1.5 1.5 0 0 1 11 4v3a1.5 1.5 0 0 1-1.5 1.5H6L3.5 10.5V8.5A1.5 1.5 0 0 1 2 7V4z" />
      <path d="M6.5 8.5V9a1.5 1.5 0 0 0 1.5 1.5h2.5L13 12.5v-2A1.5 1.5 0 0 0 14 9V6a1.5 1.5 0 0 0-1.5-1.5H11" />
    </svg>
  ),
  // Model = a chip glyph for the LLM provider/model picker.
  model: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <rect x="4.5" y="4.5" width="7" height="7" rx="1" />
      <path d="M6.5 1.5v2M9.5 1.5v2M6.5 12.5v2M9.5 12.5v2M1.5 6.5h2M1.5 9.5h2M12.5 6.5h2M12.5 9.5h2" />
    </svg>
  ),
  // Integration = link (formerly Connection); covers hooks, binary, sources, Feishu.
  integration: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M6 10l4-4" />
      <path d="M9.5 3.5l1-1a2.12 2.12 0 0 1 3 3l-1 1M6.5 12.5l-1 1a2.12 2.12 0 0 1-3-3l1-1" />
    </svg>
  ),
  usage: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M2 14V3" />
      <path d="M2 14h12" />
      <path d="M5 11l2.5-3L10 10l3-4" />
    </svg>
  ),
};

export function SettingsPanel({ onClose, standalone = false }: { onClose: () => void; standalone?: boolean }) {
  const { t } = useTranslation();
  const { connection, disconnect } = useConnectionStore();
  const [activeTab, setActiveTab] = useState<SettingsTab>("general");
  // Advanced group starts expanded only when an advanced tab is somehow the
  // initial tab; otherwise everyday users see just the 3 base tabs.
  const [showAdvanced, setShowAdvanced] = useState(false);

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

  // ── Claude binary picker ────────────────────────────────────────────────
  const [claudeBinaries, setClaudeBinaries] = useState<ClaudeBinary[]>([]);
  const [claudeBinaryOverride, setClaudeBinaryOverrideState] = useState<string>("");
  const [claudeBinaryNeedRestart, setClaudeBinaryNeedRestart] = useState(false);
  // The raw path picker stays collapsed unless the user already has a manual
  // override set — auto-detect covers everyone else.
  const [showBinaryPicker, setShowBinaryPicker] = useState(false);

  useEffect(() => {
    invoke<ClaudeBinary[]>("list_claude_binaries").then(setClaudeBinaries).catch(() => {});
    invoke<string | null>("get_claude_binary_override")
      .then((p) => {
        setClaudeBinaryOverrideState(p ?? "");
        if (p) setShowBinaryPicker(true);
      })
      .catch(() => {});
  }, []);

  const handleClaudeBinaryChange = useCallback(async (value: string) => {
    setClaudeBinaryOverrideState(value);
    try {
      await invoke("set_claude_binary_override", {
        path: value === "" ? null : value,
      });
      setClaudeBinaryNeedRestart(true);
    } catch {
      // ignore
    }
  }, []);

  // ── Hooks state ──────────────────────────────────────────────────────────
  const [hooksPlan, setHooksPlan] = useState<HookSetupPlan | null>(null);
  const [hooksStatus, setHooksStatus] = useState<"idle" | "installing" | "success" | "error">("idle");
  const [hooksError, setHooksError] = useState("");

  useEffect(() => {
    invoke<HookSetupPlan>("get_hooks_setup_plan").then((plan) => {
      setHooksPlan(plan);
      // Auto-apply hooks that the UI shows as enabled but were never actually installed
      // (e.g. user dismissed onboarding without toggling the default-on checkboxes)
      if (getItem("guard-enabled") !== "false" && !plan.guardInstalled) {
        invoke("apply_guard_hook").catch((e: unknown) =>
          console.error("auto-apply guard hook:", e),
        );
      }
      if (getItem("elicitation-enabled") !== "false" && !plan.elicitationInstalled) {
        invoke("apply_elicitation_hook").catch((e: unknown) =>
          console.error("auto-apply elicitation hook:", e),
        );
      }
      // Sync the toggle to actual disk state — localStorage gets wiped on
      // reinstall/cache-clear, but the sentinel block in ~/.claude/CLAUDE.md
      // is the source of truth.
      setInteractionModeEnabled(plan.interactionModeInstalled);
      setItem(
        "interaction-mode-enabled",
        plan.interactionModeInstalled ? "true" : "false",
      );
      // Plan approval: opt-in, so disk is the source of truth (same as
      // interaction mode). No auto-apply on mount.
      setPlanApprovalEnabled(plan.planApprovalInstalled);
      setItem(
        "plan-approval-enabled",
        plan.planApprovalInstalled ? "true" : "false",
      );
      // Re-apply on startup if installed, to pick up any title/locale changes
      // made while Fleet was closed.
      if (plan.interactionModeInstalled) {
        invoke("apply_interaction_mode").catch((e: unknown) =>
          console.error("auto-apply interaction mode:", e),
        );
      }
      // PRD discipline: opt-in, disk is the source of truth.
      const prdInstalled = plan.prdDisciplineInstalled && plan.prdContextInstalled;
      setPrdModeEnabled(prdInstalled);
      setItem("prd-mode-enabled", prdInstalled ? "true" : "false");
      if (prdInstalled) {
        invoke("apply_prd_mode").catch((e: unknown) =>
          console.error("auto-apply prd mode:", e),
        );
      }
      // Wiki guidance: opt-in, disk is the source of truth.
      setWikiGuidanceEnabled(plan.wikiGuidanceInstalled);
      setItem("wiki-guidance-enabled", plan.wikiGuidanceInstalled ? "true" : "false");
      if (plan.wikiGuidanceInstalled) {
        invoke("apply_wiki_guidance").catch((e: unknown) =>
          console.error("auto-apply wiki guidance:", e),
        );
      }
    }).catch(() => {});
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

  // ── Guard state ────────────────────────────────────────────────────────
  const [guardEnabled, setGuardEnabled] = useState(
    () => getItem("guard-enabled") !== "false",
  );
  const [guardLlmAnalysis, setGuardLlmAnalysis] = useState(
    () => getItem("guard-llm-analysis") !== "false",
  );

  const handleToggleGuard = useCallback(async (enabled: boolean) => {
    setGuardEnabled(enabled);
    setItem("guard-enabled", enabled ? "true" : "false");
    try {
      if (enabled) {
        await invoke("apply_guard_hook");
      } else {
        await invoke("remove_guard_hook");
      }
      // Refresh hooks plan to reflect guard_installed state.
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("guard hook toggle failed:", e);
    }
  }, []);

  const handleToggleGuardLlm = useCallback((enabled: boolean) => {
    setGuardLlmAnalysis(enabled);
    setItem("guard-llm-analysis", enabled ? "true" : "false");
  }, []);

  // ── Permissions bypass state (cross-process, persists to ~/.fleet) ────
  interface PermissionsConfig {
    enabled: boolean;
  }
  const [permissionsBypassEnabled, setPermissionsBypassEnabled] = useState<boolean | null>(
    null,
  );

  useEffect(() => {
    invoke<PermissionsConfig>("get_permissions_config")
      .then((cfg) => setPermissionsBypassEnabled(cfg.enabled))
      .catch(() => setPermissionsBypassEnabled(true));
  }, []);

  const handleTogglePermissionsBypass = useCallback(async (enabled: boolean) => {
    setPermissionsBypassEnabled(enabled);
    try {
      await invoke<PermissionsConfig>("set_permissions_config", {
        cfg: { enabled },
      });
    } catch (e) {
      console.error("set_permissions_config failed:", e);
      setPermissionsBypassEnabled(!enabled);
    }
  }, []);

  // ── Elicitation state ─────────────────────────────────────────────────
  const [elicitationEnabled, setElicitationEnabled] = useState(
    () => getItem("elicitation-enabled") !== "false",
  );

  // ── Interaction mode state ────────────────────────────────────────────
  const [interactionModeEnabled, setInteractionModeEnabled] = useState(
    () => getItem("interaction-mode-enabled") === "true",
  );

  const handleToggleInteractionMode = useCallback(async (enabled: boolean) => {
    setInteractionModeEnabled(enabled);
    setItem("interaction-mode-enabled", enabled ? "true" : "false");
    try {
      if (enabled) {
        await invoke("apply_interaction_mode");
      } else {
        await invoke("remove_interaction_mode");
      }
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("interaction mode toggle failed:", e);
    }
  }, []);

  // ── PRD discipline mode state (default off) ───────────────────────────
  const [prdModeEnabled, setPrdModeEnabled] = useState(
    () => getItem("prd-mode-enabled") === "true",
  );

  const handleTogglePrdMode = useCallback(async (enabled: boolean) => {
    setPrdModeEnabled(enabled);
    setItem("prd-mode-enabled", enabled ? "true" : "false");
    try {
      if (enabled) {
        await invoke("apply_prd_mode");
      } else {
        await invoke("remove_prd_mode");
      }
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("prd mode toggle failed:", e);
    }
  }, []);

  // ── Wiki guidance state (default off) ──────────────────────────────────
  const [wikiGuidanceEnabled, setWikiGuidanceEnabled] = useState(
    () => getItem("wiki-guidance-enabled") === "true",
  );

  const handleToggleWikiGuidance = useCallback(async (enabled: boolean) => {
    setWikiGuidanceEnabled(enabled);
    setItem("wiki-guidance-enabled", enabled ? "true" : "false");
    try {
      if (enabled) {
        await invoke("apply_wiki_guidance");
      } else {
        await invoke("remove_wiki_guidance");
      }
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("wiki guidance toggle failed:", e);
    }
  }, []);

  const handleToggleElicitation = useCallback(async (enabled: boolean) => {
    setElicitationEnabled(enabled);
    setItem("elicitation-enabled", enabled ? "true" : "false");
    try {
      if (enabled) {
        await invoke("apply_elicitation_hook");
      } else {
        await invoke("remove_elicitation_hook");
        // Interaction mode depends on elicitation; disable it together.
        if (getItem("interaction-mode-enabled") === "true") {
          setInteractionModeEnabled(false);
          setItem("interaction-mode-enabled", "false");
          await invoke("remove_interaction_mode").catch(() => {});
        }
      }
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("elicitation hook toggle failed:", e);
    }
  }, []);

  // ── Plan approval state (default off) ─────────────────────────────────
  const [planApprovalEnabled, setPlanApprovalEnabled] = useState(
    () => getItem("plan-approval-enabled") === "true",
  );

  const handleTogglePlanApproval = useCallback(async (enabled: boolean) => {
    setPlanApprovalEnabled(enabled);
    setItem("plan-approval-enabled", enabled ? "true" : "false");
    try {
      if (enabled) {
        await invoke("apply_plan_approval_hook");
      } else {
        await invoke("remove_plan_approval_hook");
      }
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("plan-approval hook toggle failed:", e);
    }
  }, []);

  // ── Decision panel timeouts (cross-process, persists to ~/.fleet) ─────
  interface DecisionPanelConfig {
    wait_seconds: number;
    poll_ms: number;
    heartbeat_window_seconds: number;
  }
  const [timeouts, setTimeouts] = useState<DecisionPanelConfig | null>(null);
  // Edit-then-commit-on-blur pattern: free typing during editing, persist
  // (and clamp via Rust) only when the input loses focus. Avoids saving
  // mid-keystroke values like "6" while the user is heading toward "600".
  const [timeoutsDraft, setTimeoutsDraft] = useState<{
    wait_seconds: string;
    poll_ms: string;
    heartbeat_window_seconds: string;
  } | null>(null);

  useEffect(() => {
    invoke<DecisionPanelConfig>("get_decision_panel_config")
      .then((cfg) => {
        setTimeouts(cfg);
        setTimeoutsDraft({
          wait_seconds: String(cfg.wait_seconds),
          poll_ms: String(cfg.poll_ms),
          heartbeat_window_seconds: String(cfg.heartbeat_window_seconds),
        });
      })
      .catch(() => {});
  }, []);

  const commitTimeoutField = useCallback(
    async (field: keyof DecisionPanelConfig) => {
      if (!timeouts || !timeoutsDraft) return;
      const raw = timeoutsDraft[field];
      const parsed = parseInt(raw, 10);
      if (!Number.isFinite(parsed)) {
        // Bad input — revert to last good value.
        setTimeoutsDraft({
          wait_seconds: String(timeouts.wait_seconds),
          poll_ms: String(timeouts.poll_ms),
          heartbeat_window_seconds: String(timeouts.heartbeat_window_seconds),
        });
        return;
      }
      const next = { ...timeouts, [field]: parsed };
      try {
        // Rust clamps to valid range and returns the clamped value; sync
        // both state and draft so the UI shows what's actually on disk.
        const saved = await invoke<DecisionPanelConfig>(
          "set_decision_panel_config",
          { cfg: next },
        );
        setTimeouts(saved);
        setTimeoutsDraft({
          wait_seconds: String(saved.wait_seconds),
          poll_ms: String(saved.poll_ms),
          heartbeat_window_seconds: String(saved.heartbeat_window_seconds),
        });
      } catch (e) {
        console.error("set_decision_panel_config failed:", e);
        setTimeoutsDraft({
          wait_seconds: String(timeouts.wait_seconds),
          poll_ms: String(timeouts.poll_ms),
          heartbeat_window_seconds: String(timeouts.heartbeat_window_seconds),
        });
      }
    },
    [timeouts, timeoutsDraft],
  );

  // ── Feishu (Lark) Decision Panel mirror ─────────────────────────────────
  type FeishuConnection =
    | { kind: "not_connected" }
    | { kind: "connected"; open_id: string; name: string | null };
  type OauthHandle = { state: string; authorizeUrl: string; port: number };
  type OauthStatus =
    | { kind: "pending" }
    | { kind: "connected"; open_id: string; name: string | null }
    | { kind: "failed"; reason: string };

  const [feishuConn, setFeishuConn] = useState<FeishuConnection>({ kind: "not_connected" });
  const [feishuAuthorizing, setFeishuAuthorizing] = useState(false);
  const [feishuError, setFeishuError] = useState<string | null>(null);

  useEffect(() => {
    if (activeTab !== "integration") return;
    invoke<FeishuConnection>("feishu_status").then(setFeishuConn).catch(() => {});
  }, [activeTab]);

  const handleConnectFeishu = useCallback(async () => {
    setFeishuError(null);
    setFeishuAuthorizing(true);
    try {
      const handle = await invoke<OauthHandle>("connect_feishu");
      const { openUrl } = await import("@tauri-apps/plugin-opener");
      await openUrl(handle.authorizeUrl);
      const oauthState = handle.state;
      const started = Date.now();
      const tick = async (): Promise<void> => {
        if (Date.now() - started > 5 * 60 * 1000) {
          setFeishuAuthorizing(false);
          setFeishuError("oauth flow timed out");
          return;
        }
        try {
          const status = await invoke<OauthStatus>("poll_feishu_oauth", { oauthState });
          if (status.kind === "pending") {
            setTimeout(tick, 1500);
            return;
          }
          if (status.kind === "connected") {
            setFeishuConn({ kind: "connected", open_id: status.open_id, name: status.name });
          } else {
            setFeishuError(status.reason);
          }
        } catch (e: unknown) {
          setFeishuError(typeof e === "string" ? e : String(e));
        }
        setFeishuAuthorizing(false);
      };
      setTimeout(tick, 1500);
    } catch (e: unknown) {
      setFeishuAuthorizing(false);
      setFeishuError(typeof e === "string" ? e : String(e));
    }
  }, []);

  const handleDisconnectFeishu = useCallback(async () => {
    try {
      await invoke("disconnect_feishu");
      setFeishuConn({ kind: "not_connected" });
      setFeishuError(null);
    } catch (e: unknown) {
      setFeishuError(typeof e === "string" ? e : String(e));
    }
  }, []);

  // ── QA mode diagnostics ───────────────────────────────────────────────
  type DiagnosticCheck = {
    id: string;
    label: string;
    status: "pass" | "warn" | "fail" | "unknown";
    detail: string;
    fixAction?:
      | "reinstall_interaction_mode"
      | "enable_elicitation_hook"
      | "enable_mcp_injector";
  };
  type TestRunResult = {
    kind: string;
    requestId?: string;
    message: string;
    claudeOutput?: string;
  };
  const TEST_WORKSPACE_MARKER = "[QA Diagnostic Test]";

  const [interactionChecks, setInteractionChecks] = useState<DiagnosticCheck[]>([]);
  const [frontendListenerCheck, setFrontendListenerCheck] = useState<{
    status: "pass" | "unknown";
    detail: string;
  }>({ status: "unknown", detail: "" });
  const [testingKind, setTestingKind] = useState<
    null | "frontend" | "e2e" | "cli" | "fleet_ask_e2e" | "fleet_ask_cli"
  >(null);
  const [lastTestResult, setLastTestResult] = useState<TestRunResult | null>(null);
  // Consumer-facing diagnostics: a single status card + one-click fix. The raw
  // per-check detail and the deep test buttons live behind this collapsed
  // expander so a normal user never meets the debug-log surface.
  const [showAdvancedDiagnostics, setShowAdvancedDiagnostics] = useState(false);
  const [fixingAll, setFixingAll] = useState(false);
  // Raw timing parameters are tucked behind this collapsed expander at the
  // bottom of the Interaction tab — most users never need them.
  const [showInteractionAdvanced, setShowInteractionAdvanced] = useState(false);

  const refreshInteractionDiagnostics = useCallback(async () => {
    try {
      const checks = await invoke<DiagnosticCheck[]>("get_interaction_diagnostics");
      setInteractionChecks(checks);
    } catch (e) {
      console.error("get_interaction_diagnostics failed:", e);
    }
  }, []);

  useEffect(() => {
    if (activeTab !== "interaction") return;
    refreshInteractionDiagnostics();
  }, [activeTab, refreshInteractionDiagnostics]);

  // The frontend listener row can only be verified by observing one of our
  // test cards actually arrive on the elicitation-request channel — there's
  // no backend-side signal for "listener attached". Keep this mounted at all
  // times so a test triggered on one tab still records when the event hits.
  useEffect(() => {
    let unlistenFn: (() => void) | undefined;
    (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      unlistenFn = await listen<{ workspaceName?: string }>(
        "elicitation-request",
        (event) => {
          if (event.payload?.workspaceName === TEST_WORKSPACE_MARKER) {
            setFrontendListenerCheck({
              status: "pass",
              detail: `Last test card received ${new Date().toLocaleTimeString()}`,
            });
          }
        },
      );
    })();
    return () => unlistenFn?.();
  }, []);

  const runDiagnosticTest = useCallback(
    async (kind: "frontend" | "e2e" | "cli" | "fleet_ask_e2e" | "fleet_ask_cli") => {
      setTestingKind(kind);
      setLastTestResult(null);
      const cmd =
        kind === "frontend"
          ? "test_decision_frontend_only"
          : kind === "e2e"
            ? "test_decision_end_to_end"
            : kind === "cli"
              ? "test_decision_via_claude_cli"
              : kind === "fleet_ask_e2e"
                ? "test_fleet_ask_end_to_end"
                : "test_fleet_ask_via_claude_cli";
      try {
        const r = await invoke<TestRunResult>(cmd);
        setLastTestResult(r);
      } catch (e: unknown) {
        setLastTestResult({
          kind,
          message: typeof e === "string" ? e : String(e),
        });
      } finally {
        setTestingKind(null);
      }
    },
    [],
  );

  const handleInteractionFix = useCallback(
    async (
      action:
        | "reinstall_interaction_mode"
        | "enable_elicitation_hook"
        | "enable_mcp_injector",
    ) => {
      try {
        if (action === "reinstall_interaction_mode") {
          await invoke("apply_interaction_mode");
        } else if (action === "enable_elicitation_hook") {
          await invoke("apply_elicitation_hook");
        } else {
          await invoke("apply_mcp_injector");
        }
        await refreshInteractionDiagnostics();
      } catch (e) {
        console.error("fix failed:", e);
      }
    },
    [refreshInteractionDiagnostics],
  );

  // Run every distinct fix action across all currently-failing checks, then
  // re-check. Deduped because reinstall_interaction_mode covers both the
  // CLAUDE.md sentinel and the guidance file.
  const handleFixAll = useCallback(async () => {
    setFixingAll(true);
    try {
      const actions = Array.from(
        new Set(
          interactionChecks
            .filter((c) => c.status === "fail" || c.status === "warn")
            .map((c) => c.fixAction)
            .filter((a): a is NonNullable<DiagnosticCheck["fixAction"]> => !!a),
        ),
      );
      for (const action of actions) {
        if (action === "reinstall_interaction_mode") {
          await invoke("apply_interaction_mode");
        } else if (action === "enable_elicitation_hook") {
          await invoke("apply_elicitation_hook");
        } else if (action === "enable_mcp_injector") {
          await invoke("apply_mcp_injector");
        }
      }
      await refreshInteractionDiagnostics();
    } catch (e) {
      console.error("fix-all failed:", e);
    } finally {
      setFixingAll(false);
    }
  }, [interactionChecks, refreshInteractionDiagnostics]);

  const statusIcon = (s: DiagnosticCheck["status"]) =>
    s === "pass" ? "✅" : s === "warn" ? "⚠️" : s === "fail" ? "❌" : "❓";

  type FeishuStoredCreds = {
    app_id: string;
    app_secret: string;
    encrypt_key: string;
    verification_token: string;
  };
  const [feishuCreds, setFeishuCreds] = useState<FeishuStoredCreds>({
    app_id: "",
    app_secret: "",
    encrypt_key: "",
    verification_token: "",
  });
  const [feishuCredsSaving, setFeishuCredsSaving] = useState(false);
  const [feishuCredsSaved, setFeishuCredsSaved] = useState(false);
  // Tracks form edits not yet persisted. `connect_feishu` reads creds from
  // disk, so the "Connect" button must stay disabled until the App ID/Secret
  // the user typed are actually saved — otherwise verify silently runs against
  // stale on-disk creds.
  const [feishuCredsDirty, setFeishuCredsDirty] = useState(false);
  // In the connected state the raw credential fields collapse behind a toggle;
  // while not connected they stay open so setup leads with the form.
  const [showFeishuCreds, setShowFeishuCreds] = useState(false);

  useEffect(() => {
    if (activeTab !== "integration") return;
    invoke<FeishuStoredCreds>("get_feishu_creds")
      .then((c) => {
        setFeishuCreds(c);
        setFeishuCredsDirty(false);
      })
      .catch(() => {});
  }, [activeTab]);

  const handleSaveFeishuCreds = useCallback(async () => {
    setFeishuError(null);
    setFeishuCredsSaved(false);
    setFeishuCredsSaving(true);
    try {
      await invoke("set_feishu_creds", { creds: feishuCreds });
      setFeishuCredsSaved(true);
      setFeishuCredsDirty(false);
    } catch (e: unknown) {
      setFeishuError(typeof e === "string" ? e : String(e));
    } finally {
      setFeishuCredsSaving(false);
    }
  }, [feishuCreds]);

  // On-disk App ID + Secret both present and no unsaved edits → safe to verify.
  const feishuCanVerify =
    feishuCreds.app_id.trim() !== "" &&
    feishuCreds.app_secret.trim() !== "" &&
    !feishuCredsDirty;
  // While not connected the credential form stays open (setup leads with it);
  // once connected it tucks behind the "edit credentials" toggle.
  const feishuCredsFormVisible =
    feishuConn.kind !== "connected" || showFeishuCreds;

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

  // ── Master mute ───────────────────────────────────────────────────────
  // `tts-muted` is read by both the front-end decision-panel queue
  // (audio.ts playDecisionAlert/playAlertSound) and the Rust notification
  // TTS path (gui.rs play_tts_for_notification). It used to have NO settings
  // UI — only the Lite-mode top-bar button — which is why "I turned sound
  // off but the decision panel still spoke" happened. Surface it here as the
  // single master switch.
  const [ttsMuted, setTtsMuted] = useState(() => getItem("tts-muted") === "true");

  const handleTtsMutedChange = useCallback((muted: boolean) => {
    setTtsMuted(muted);
    setItem("tts-muted", muted ? "true" : "false");
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
    () => getItem("personalized-mascot") === "true",
  );

  const handleTogglePersonalizedMascot = useCallback((enabled: boolean) => {
    setPersonalizedMascot(enabled);
    setItem("personalized-mascot", enabled ? "true" : "false");
  }, []);

  // ── Mascot visibility state ────────────────────────────────────────────
  const [mascotVisible, setMascotVisibleLocal] = useState(
    () => getItem("mascot-visible") === "true",
  );

  const handleToggleMascotVisible = useCallback(async (enabled: boolean) => {
    setMascotVisibleLocal(enabled);
    setItem("mascot-visible", enabled ? "true" : "false");
    const { emit } = await import("@tauri-apps/api/event");
    await emit("overlay-mascot-visible-changed", enabled).catch(() => {});
  }, []);

  // ── Floating decision panel state ──────────────────────────────────────
  const [floatingDecisionPanel, setFloatingDecisionPanelLocal] = useState(
    () => getItem("floating-decision-panel") === "true",
  );

  const handleToggleFloatingDecisionPanel = useCallback(async (enabled: boolean) => {
    setFloatingDecisionPanelLocal(enabled);
    setItem("floating-decision-panel", enabled ? "true" : "false");
    const { emit } = await import("@tauri-apps/api/event");
    await emit("overlay-floating-decision-panel-changed", enabled).catch(() => {});
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

  // ── Auto-resume config ──────────────────────────────────────────────────
  const [autoResume, setAutoResume] = useState<{ enabled: boolean; maxWaitHours: number }>({
    enabled: true,
    maxWaitHours: 12,
  });

  useEffect(() => {
    invoke<{ enabled: boolean; maxWaitHours: number }>("get_auto_resume_config")
      .then(setAutoResume)
      .catch(() => {});
  }, []);

  const handleAutoResumeChange = useCallback(
    (patch: Partial<{ enabled: boolean; maxWaitHours: number }>) => {
      setAutoResume((prev) => {
        const next = { ...prev, ...patch };
        invoke("set_auto_resume_config", { config: next }).catch(() => {});
        return next;
      });
    },
    [],
  );

  // ── Keep-awake (caffeinate -i equivalent) ───────────────────────────────
  const { enabled: keepAwake, supported: keepAwakeSupported, setKeepAwake } = useKeepAwake();

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
    if (standalone) {
      // Defer to the main window; it owns the connection + detail stores.
      const { emit } = await import("@tauri-apps/api/event");
      await emit("switch-connection").catch(() => {});
      onClose();
      return;
    }
    await useDetailStore.getState().close();
    await disconnect();
    onClose();
  }, [disconnect, onClose, standalone]);

  const hooksInstalled = hooksPlan?.alreadyInstalled || hooksStatus === "success";

  const tabLabels: Record<SettingsTab, string> = {
    general: t("settings.tab_general"),
    alerts: t("settings.tab_alerts"),
    account: t("settings.tab_account"),
    interaction: t("settings.tab_interaction"),
    model: t("settings.tab_model"),
    integration: t("settings.tab_integration"),
    usage: t("settings.usage"),
  };
  const renderTabButton = (key: SettingsTab) => (
    <button
      key={key}
      className={`${styles.menu_item} ${activeTab === key ? styles.menu_item_active : ""}`}
      onClick={() => setActiveTab(key)}
    >
      {tabIcons[key]}
      {tabLabels[key]}
    </button>
  );

  const wrapperProps = standalone
    ? { className: styles.standalone_root }
    : { className: styles.overlay, onClick: onClose };
  const panelClass = standalone ? styles.standalone_panel : styles.panel;

  return (
    <div {...wrapperProps}>
      <div className={panelClass} onClick={(e) => e.stopPropagation()}>
        {/* Header */}
        <div className={styles.header}>
          <h2 className={styles.title}>{t("settings.title")}</h2>
          <button className={styles.close_btn} onClick={onClose}>
            {t("settings.close")}
          </button>
        </div>

        <div className={styles.body}>
          {/* ── Left: menu — everyday tabs, then a collapsible Advanced group ── */}
          <nav className={styles.menu}>
            {BASE_TABS.map(renderTabButton)}

            <button
              type="button"
              className={styles.menu_group_header}
              onClick={() => setShowAdvanced((v) => !v)}
            >
              <span aria-hidden>{showAdvanced ? "▾" : "▸"}</span>
              {t("settings.group_advanced")}
            </button>
            {showAdvanced && ADVANCED_TABS.map(renderTabButton)}
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

                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.auto_resume")}</span>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                      {t("settings.auto_resume_desc")}
                    </span>
                  </div>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={autoResume.enabled}
                      onChange={(e) => handleAutoResumeChange({ enabled: e.target.checked })}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>
                {autoResume.enabled && (
                  <div className={styles.row}>
                    <div>
                      <span className={styles.row_label}>{t("settings.auto_resume_max_wait")}</span>
                      <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                        {t("settings.auto_resume_max_wait_desc")}
                      </span>
                    </div>
                    <input
                      type="number"
                      min={1}
                      max={168}
                      value={autoResume.maxWaitHours}
                      onChange={(e) => {
                        const n = parseInt(e.target.value, 10);
                        if (!isNaN(n) && n > 0) handleAutoResumeChange({ maxWaitHours: n });
                      }}
                      style={{ width: 72 }}
                    />
                  </div>
                )}

                {keepAwakeSupported && (
                  <div className={styles.row}>
                    <div>
                      <span className={styles.row_label}>{t("settings.keep_awake")}</span>
                      <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                        {t("settings.keep_awake_desc")}
                      </span>
                    </div>
                    <label className={styles.toggle}>
                      <input
                        type="checkbox"
                        checked={keepAwake}
                        onChange={(e) => setKeepAwake(e.target.checked)}
                      />
                      <span className={styles.toggle_slider} />
                    </label>
                  </div>
                )}

              </div>
            )}

            {/* ── Model (advanced) — LLM provider & models ── */}
            {activeTab === "model" && (
              <div className={styles.section}>
                <div className={styles.section_title}>
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

            {/* ── Appearance (merged into General) ── */}
            {activeTab === "general" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.appearance")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.theme")}</span>
                  <ThemeToggle />
                </div>
                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.mascot_visible")}</span>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                      {t("settings.mascot_visible_desc")}
                    </span>
                  </div>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={mascotVisible}
                      onChange={(e) => handleToggleMascotVisible(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>
                {mascotVisible && (
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
                )}
              </div>
            )}

            {/* ── Profile (merged into General) ── */}
            {activeTab === "general" && (
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

            {/* ── Account & Connection ── */}
            {activeTab === "account" && (
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

            {/* ── Integration (advanced): hooks + Claude binary + sources + Feishu ── */}
            {activeTab === "integration" && (
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

                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.claude_binary")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.claude_binary_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <button
                    type="button"
                    onClick={() => setShowBinaryPicker((v) => !v)}
                    style={{
                      background: "none",
                      border: "none",
                      padding: 0,
                      cursor: "pointer",
                      fontSize: 12,
                      color: "var(--color-text-dim)",
                      display: "flex",
                      alignItems: "center",
                      gap: 4,
                    }}
                  >
                    <span aria-hidden>{showBinaryPicker ? "▾" : "▸"}</span>
                    {t("settings.claude_binary_advanced_toggle")}
                  </button>
                </div>
                {showBinaryPicker && (
                  <>
                    <div className={styles.row}>
                      <span className={styles.row_label}>{t("settings.claude_binary_picker")}</span>
                      <select
                        className={styles.select}
                        value={claudeBinaryOverride}
                        onChange={(e) => handleClaudeBinaryChange(e.target.value)}
                      >
                        <option value="">{t("settings.claude_binary_auto")}</option>
                        {claudeBinaries.map((b) => (
                          <option key={b.path} value={b.path}>
                            {t(`settings.claude_binary_source.${b.source}`)}
                            {b.version ? ` ${b.version}` : ""}
                            {" — "}{b.path}
                          </option>
                        ))}
                      </select>
                    </div>
                    {claudeBinaryOverride && !claudeBinaries.some((b) => b.path === claudeBinaryOverride) && (
                      <div className={styles.row}>
                        <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                          {t("settings.claude_binary_custom_path", { path: claudeBinaryOverride })}
                        </span>
                      </div>
                    )}
                  </>
                )}
                {claudeBinaryNeedRestart && (
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

            {/* ── Interaction ── */}
            {activeTab === "interaction" && (
              <>
              {/* ── Group: how decisions reach you (floating panel toggle moved to Alerts) ── */}
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.interaction_group_decision")}</div>

                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.elicitation")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.elicitation_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.elicitation_enabled")}</span>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={elicitationEnabled}
                      onChange={(e) => handleToggleElicitation(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>

                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.plan_approval")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.plan_approval_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.plan_approval_enabled")}</span>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={planApprovalEnabled}
                      onChange={(e) => handleTogglePlanApproval(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>

                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.interaction_mode")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.interaction_mode_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.interaction_mode_enabled")}</span>
                    {!elicitationEnabled && (
                      <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                        {t("settings.interaction_mode_requires_elicitation")}
                      </span>
                    )}
                  </div>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={interactionModeEnabled}
                      disabled={!elicitationEnabled}
                      onChange={(e) => handleToggleInteractionMode(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>
              </div>

              {/* ── Group: safety ── */}
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.interaction_group_safety")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.guard_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.guard_enabled")}</span>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={guardEnabled}
                      onChange={(e) => handleToggleGuard(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.guard_llm_analysis")}</span>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={guardLlmAnalysis}
                      disabled={!guardEnabled}
                      onChange={(e) => handleToggleGuardLlm(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>

                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.permissions_bypass")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.permissions_bypass_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.permissions_bypass_enabled")}</span>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={permissionsBypassEnabled ?? true}
                      disabled={permissionsBypassEnabled === null}
                      onChange={(e) => handleTogglePermissionsBypass(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>
              </div>

              {/* ── Diagnostics ── */}
              <div className={styles.section}>
                <div className={styles.section_title}>
                  {t("settings.interaction_diagnostics")}
                </div>
                <div className={styles.row}>
                  <span
                    className={styles.row_label}
                    style={{ fontSize: 11, color: "var(--color-text-dim)" }}
                  >
                    {t("settings.interaction_diagnostics_desc")}
                  </span>
                </div>

                {(() => {
                  const problems = interactionChecks.filter(
                    (c) => c.status === "fail" || c.status === "warn",
                  );
                  const fixable = problems.filter((c) => c.fixAction);
                  const ok = problems.length === 0;
                  return (
                    <div
                      className={styles.row}
                      style={{
                        flexDirection: "column",
                        alignItems: "stretch",
                        gap: 10,
                        padding: "14px 16px",
                        borderRadius: 10,
                        border: `1px solid ${ok ? "var(--color-success-border, rgba(52,199,89,0.35))" : "var(--color-warning-border, rgba(255,159,10,0.4))"}`,
                        background: ok
                          ? "var(--color-success-bg, rgba(52,199,89,0.08))"
                          : "var(--color-warning-bg, rgba(255,159,10,0.08))",
                      }}
                    >
                      <div style={{ display: "flex", alignItems: "center", gap: 10 }}>
                        <span style={{ fontSize: 22 }} aria-hidden>
                          {ok ? "✅" : "⚠️"}
                        </span>
                        <div style={{ display: "flex", flexDirection: "column", gap: 2 }}>
                          <span style={{ fontSize: 14, fontWeight: 600, color: "var(--color-text)" }}>
                            {ok
                              ? t("settings.interaction_diagnostics_status_ok_title")
                              : t("settings.interaction_diagnostics_status_problem_title", {
                                  count: problems.length,
                                })}
                          </span>
                          <span style={{ fontSize: 12, color: "var(--color-text-dim)" }}>
                            {ok
                              ? t("settings.interaction_diagnostics_status_ok_desc")
                              : t("settings.interaction_diagnostics_status_problem_desc")}
                          </span>
                        </div>
                      </div>

                      {!ok && (
                        <ul
                          style={{
                            margin: 0,
                            paddingLeft: 44,
                            display: "flex",
                            flexDirection: "column",
                            gap: 4,
                          }}
                        >
                          {problems.map((c) => (
                            <li key={c.id} style={{ fontSize: 12, color: "var(--color-text)" }}>
                              {t(`settings.interaction_diagnostics_problem_${c.id}`, {
                                defaultValue: c.label,
                              })}
                              {!c.fixAction && (
                                <span style={{ fontSize: 11, color: "var(--color-text-dim)", marginLeft: 6 }}>
                                  {t("settings.interaction_diagnostics_problem_manual")}
                                </span>
                              )}
                            </li>
                          ))}
                        </ul>
                      )}

                      <div style={{ display: "flex", gap: 8, marginLeft: 32 }}>
                        {!ok && fixable.length > 0 && (
                          <button
                            type="button"
                            onClick={handleFixAll}
                            disabled={fixingAll}
                            className={styles.hooks_install_btn}
                            style={{ fontSize: 12, padding: "4px 14px" }}
                          >
                            {fixingAll
                              ? t("settings.interaction_diagnostics_fixing")
                              : t("settings.interaction_diagnostics_fix_all")}
                          </button>
                        )}
                        <button
                          type="button"
                          onClick={refreshInteractionDiagnostics}
                          disabled={fixingAll}
                          style={{ fontSize: 12, padding: "4px 14px" }}
                        >
                          {t("settings.interaction_diagnostics_refresh")}
                        </button>
                      </div>
                    </div>
                  );
                })()}

                <div className={styles.row} style={{ marginTop: 4 }}>
                  <button
                    type="button"
                    onClick={() => setShowAdvancedDiagnostics((v) => !v)}
                    style={{
                      background: "none",
                      border: "none",
                      padding: 0,
                      cursor: "pointer",
                      fontSize: 12,
                      color: "var(--color-text-dim)",
                      display: "flex",
                      alignItems: "center",
                      gap: 4,
                    }}
                  >
                    <span aria-hidden>{showAdvancedDiagnostics ? "▾" : "▸"}</span>
                    {t("settings.interaction_diagnostics_advanced")}
                  </button>
                </div>

                {showAdvancedDiagnostics && (
                <>
                {[
                  ...interactionChecks,
                  {
                    id: "frontend_listener",
                    label: "Frontend listener",
                    status: frontendListenerCheck.status,
                    detail:
                      frontendListenerCheck.status === "pass"
                        ? frontendListenerCheck.detail
                        : t("settings.interaction_diagnostics_frontend_unknown"),
                  } as DiagnosticCheck,
                ].map((c) => (
                  <div key={c.id} className={styles.row} style={{ alignItems: "flex-start" }}>
                    <div style={{ flex: 1, minWidth: 0 }}>
                      <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                        <span aria-hidden>{statusIcon(c.status)}</span>
                        <span className={styles.row_label}>
                          {t(
                            `settings.interaction_diagnostics_check_${c.id}`,
                            { defaultValue: c.label },
                          )}
                        </span>
                      </div>
                      <div
                        style={{
                          fontSize: 11,
                          color: "var(--color-text-dim)",
                          marginTop: 2,
                          marginLeft: 22,
                          wordBreak: "break-word",
                        }}
                      >
                        {c.detail}
                      </div>
                    </div>
                    {c.fixAction && (
                      <button
                        type="button"
                        onClick={() => handleInteractionFix(c.fixAction!)}
                        style={{ fontSize: 12, padding: "2px 10px", whiteSpace: "nowrap" }}
                      >
                        {t("settings.interaction_diagnostics_fix")}
                      </button>
                    )}
                  </div>
                ))}

                <div
                  className={styles.row}
                  style={{ flexDirection: "column", alignItems: "stretch", gap: 8 }}
                >
                  {[
                    { kind: "frontend" as const, label: "interaction_diagnostics_test_frontend", hint: "interaction_diagnostics_test_frontend_hint" },
                    { kind: "e2e" as const, label: "interaction_diagnostics_test_e2e", hint: "interaction_diagnostics_test_e2e_hint" },
                    { kind: "cli" as const, label: "interaction_diagnostics_test_claude_cli", hint: "interaction_diagnostics_test_claude_cli_hint" },
                    { kind: "fleet_ask_e2e" as const, label: "interaction_diagnostics_test_fleet_ask_e2e", hint: "interaction_diagnostics_test_fleet_ask_e2e_hint" },
                    { kind: "fleet_ask_cli" as const, label: "interaction_diagnostics_test_fleet_ask_cli", hint: "interaction_diagnostics_test_fleet_ask_cli_hint" },
                  ].map((b) => (
                    <div key={b.kind} style={{ display: "flex", alignItems: "flex-start", gap: 8 }}>
                      <button
                        type="button"
                        onClick={() => runDiagnosticTest(b.kind)}
                        disabled={testingKind !== null}
                        style={{ minWidth: 180, padding: "4px 10px", fontSize: 12 }}
                      >
                        {testingKind === b.kind
                          ? t("settings.interaction_diagnostics_test_running")
                          : t(`settings.${b.label}`)}
                      </button>
                      <span
                        style={{ fontSize: 11, color: "var(--color-text-dim)", flex: 1 }}
                      >
                        {t(`settings.${b.hint}`)}
                      </span>
                    </div>
                  ))}
                </div>

                {lastTestResult && (
                  <div
                    className={styles.row}
                    style={{ flexDirection: "column", alignItems: "stretch", gap: 4 }}
                  >
                    <div style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                      <b>{t("settings.interaction_diagnostics_last_test")}:</b>{" "}
                      {lastTestResult.message}
                    </div>
                    {lastTestResult.claudeOutput && (
                      <pre
                        style={{
                          margin: 0,
                          padding: 8,
                          maxHeight: 200,
                          overflow: "auto",
                          fontSize: 10,
                          background: "var(--color-bg-elevated, rgba(0,0,0,0.04))",
                          borderRadius: 4,
                          whiteSpace: "pre-wrap",
                          wordBreak: "break-word",
                        }}
                      >
                        {lastTestResult.claudeOutput}
                      </pre>
                    )}
                  </div>
                )}
                </>
                )}

              </div>

              {/* ── Group: work discipline ── */}
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.interaction_group_discipline")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.prd_mode_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.prd_mode_enabled")}</span>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={prdModeEnabled}
                      onChange={(e) => handleTogglePrdMode(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.wiki_guidance_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.wiki_guidance_enabled")}</span>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={wikiGuidanceEnabled}
                      onChange={(e) => handleToggleWikiGuidance(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>

              </div>

              {/* ── Advanced timing parameters (collapsed) ── */}
              <div className={styles.section}>
                <div className={styles.row} style={{ marginTop: 0 }}>
                  <button
                    type="button"
                    onClick={() => setShowInteractionAdvanced((v) => !v)}
                    style={{
                      background: "none",
                      border: "none",
                      padding: 0,
                      cursor: "pointer",
                      fontSize: 12,
                      color: "var(--color-text-dim)",
                      display: "flex",
                      alignItems: "center",
                      gap: 4,
                    }}
                  >
                    <span aria-hidden>{showInteractionAdvanced ? "▾" : "▸"}</span>
                    {t("settings.interaction_advanced_toggle")}
                  </button>
                </div>
                {showInteractionAdvanced && (
                  <>
                    <div className={styles.section_title} style={{ marginTop: 12 }}>{t("settings.timeouts_section_title")}</div>
                    <div className={styles.row}>
                      <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                        {t("settings.timeouts_desc")}
                      </span>
                    </div>
                    {timeoutsDraft && (
                      <>
                        <div className={styles.row}>
                          <span className={styles.row_label}>
                            {t("settings.timeouts_wait")}
                            <span style={{ fontSize: 11, color: "var(--color-text-dim)", marginLeft: 6 }}>
                              {t("settings.timeouts_unit_seconds")} · 60–3600
                            </span>
                          </span>
                          <input
                            type="number"
                            min={60}
                            max={3600}
                            value={timeoutsDraft.wait_seconds}
                            onChange={(e) =>
                              setTimeoutsDraft((d) => (d ? { ...d, wait_seconds: e.target.value } : d))
                            }
                            onBlur={() => commitTimeoutField("wait_seconds")}
                            style={{
                              width: 90,
                              padding: "4px 6px",
                              background: "var(--color-bg-input)",
                              border: "1px solid var(--color-border, #333)",
                              borderRadius: 4,
                              color: "var(--color-text, #eee)",
                              fontFamily: "monospace",
                              fontSize: 12,
                              textAlign: "right",
                            }}
                          />
                        </div>
                        <div className={styles.row}>
                          <span className={styles.row_label}>
                            {t("settings.timeouts_poll")}
                            <span style={{ fontSize: 11, color: "var(--color-text-dim)", marginLeft: 6 }}>
                              {t("settings.timeouts_unit_ms")} · 50–1000
                            </span>
                          </span>
                          <input
                            type="number"
                            min={50}
                            max={1000}
                            value={timeoutsDraft.poll_ms}
                            onChange={(e) =>
                              setTimeoutsDraft((d) => (d ? { ...d, poll_ms: e.target.value } : d))
                            }
                            onBlur={() => commitTimeoutField("poll_ms")}
                            style={{
                              width: 90,
                              padding: "4px 6px",
                              background: "var(--color-bg-input)",
                              border: "1px solid var(--color-border, #333)",
                              borderRadius: 4,
                              color: "var(--color-text, #eee)",
                              fontFamily: "monospace",
                              fontSize: 12,
                              textAlign: "right",
                            }}
                          />
                        </div>
                        <div className={styles.row}>
                          <span className={styles.row_label}>
                            {t("settings.timeouts_heartbeat")}
                            <span style={{ fontSize: 11, color: "var(--color-text-dim)", marginLeft: 6 }}>
                              {t("settings.timeouts_unit_seconds")} · 5–60
                            </span>
                          </span>
                          <input
                            type="number"
                            min={5}
                            max={60}
                            value={timeoutsDraft.heartbeat_window_seconds}
                            onChange={(e) =>
                              setTimeoutsDraft((d) =>
                                d ? { ...d, heartbeat_window_seconds: e.target.value } : d,
                              )
                            }
                            onBlur={() => commitTimeoutField("heartbeat_window_seconds")}
                            style={{
                              width: 90,
                              padding: "4px 6px",
                              background: "var(--color-bg-input)",
                              border: "1px solid var(--color-border, #333)",
                              borderRadius: 4,
                              color: "var(--color-text, #eee)",
                              fontFamily: "monospace",
                              fontSize: 12,
                              textAlign: "right",
                            }}
                          />
                        </div>
                      </>
                    )}
                  </>
                )}
              </div>
              </>
            )}

            {/* ── Channels (Feishu / Lark) — merged into Integration ── */}
            {activeTab === "integration" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.feishu")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.feishu_desc")}
                  </span>
                </div>

                {feishuConn.kind === "connected" ? (
                  <>
                    <div className={styles.row}>
                      <span className={styles.row_label}>
                        {t("settings.feishu_connected_as")}: {feishuConn.name || feishuConn.open_id}
                      </span>
                      <button className={styles.switch_btn} onClick={handleDisconnectFeishu}>
                        {t("settings.feishu_disconnect")}
                      </button>
                    </div>
                    <div className={styles.row} style={{ marginTop: 4 }}>
                      <button
                        type="button"
                        onClick={() => setShowFeishuCreds((v) => !v)}
                        style={{
                          background: "none",
                          border: "none",
                          padding: 0,
                          cursor: "pointer",
                          fontSize: 12,
                          color: "var(--color-text-dim)",
                          display: "flex",
                          alignItems: "center",
                          gap: 4,
                        }}
                      >
                        <span aria-hidden>{showFeishuCreds ? "▾" : "▸"}</span>
                        {showFeishuCreds ? t("settings.feishu_hide_creds") : t("settings.feishu_show_creds")}
                      </button>
                    </div>
                  </>
                ) : (
                  <>
                    {/* Step 1 — backend setup checklist */}
                    <div className={styles.section_title} style={{ marginTop: 14 }}>
                      {t("settings.feishu_step_creds")}
                    </div>
                    <div className={styles.row}>
                      <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                        {t("settings.feishu_setup_title")}
                      </span>
                    </div>
                    <ol
                      style={{
                        margin: "2px 0 6px",
                        paddingLeft: 18,
                        fontSize: 11,
                        lineHeight: 1.7,
                        color: "var(--color-text-dim)",
                      }}
                    >
                      <li>{t("settings.feishu_setup_step_app")}</li>
                      <li>
                        {t("settings.feishu_setup_step_perms")}
                        <ul style={{ margin: "3px 0 0", paddingLeft: 16, listStyle: "disc" }}>
                          {FEISHU_SCOPES.map(({ scope, glossKey }) => (
                            <li key={scope}>
                              <code
                                style={{
                                  padding: "0 4px",
                                  background: "var(--color-bg-input)",
                                  border: "1px solid var(--color-border, #333)",
                                  borderRadius: 3,
                                  fontFamily: "monospace",
                                  fontSize: 11,
                                  color: "var(--color-text, #eee)",
                                  userSelect: "all",
                                }}
                              >
                                {scope}
                              </code>
                              {" — "}
                              {t(`settings.${glossKey}`)}
                            </li>
                          ))}
                        </ul>
                      </li>
                      <li>{t("settings.feishu_setup_step_event")}</li>
                      <li>
                        {t("settings.feishu_setup_step_redirect")}
                        <code
                          style={{
                            display: "inline-block",
                            marginLeft: 4,
                            padding: "1px 5px",
                            background: "var(--color-bg-input)",
                            border: "1px solid var(--color-border, #333)",
                            borderRadius: 4,
                            fontFamily: "monospace",
                            fontSize: 11,
                            color: "var(--color-text, #eee)",
                            userSelect: "all",
                          }}
                        >
                          {FEISHU_REDIRECT_URL}
                        </code>
                      </li>
                      <li>{t("settings.feishu_setup_step_fill")}</li>
                    </ol>
                  </>
                )}

                {feishuCredsFormVisible && (
                  <div className={styles.row} style={{ flexDirection: "column", alignItems: "stretch", gap: 8 }}>
                    {(["app_id", "app_secret", "encrypt_key", "verification_token"] as const).map((key) => (
                      <div key={key} style={{ display: "flex", flexDirection: "column", gap: 2 }}>
                        <label style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                          {t(`settings.feishu_${key}`)}
                        </label>
                        <input
                          type={key === "app_secret" || key === "encrypt_key" ? "password" : "text"}
                          value={feishuCreds[key]}
                          onChange={(e) => {
                            setFeishuCreds((prev) => ({ ...prev, [key]: e.target.value }));
                            setFeishuCredsSaved(false);
                            setFeishuCredsDirty(true);
                          }}
                          placeholder={key === "encrypt_key" || key === "verification_token" ? t("settings.feishu_optional") : ""}
                          spellCheck={false}
                          autoCorrect="off"
                          autoCapitalize="off"
                          style={{
                            padding: "6px 8px",
                            background: "var(--color-bg-input)",
                            border: "1px solid var(--color-border, #333)",
                            borderRadius: 4,
                            color: "var(--color-text, #eee)",
                            fontFamily: "monospace",
                            fontSize: 12,
                          }}
                        />
                      </div>
                    ))}
                    <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
                      <button
                        className={styles.hooks_install_btn}
                        onClick={handleSaveFeishuCreds}
                        disabled={feishuCredsSaving}
                      >
                        {feishuCredsSaving ? t("account.loading") : t("settings.feishu_save_creds")}
                      </button>
                      {feishuCredsSaved && (
                        <span style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                          {t("settings.feishu_creds_saved")}
                        </span>
                      )}
                    </div>
                  </div>
                )}

                {feishuConn.kind !== "connected" && (
                  <>
                    {/* Step 2 — verify / connect (gated on saved credentials) */}
                    <div className={styles.section_title} style={{ marginTop: 18 }}>
                      {t("settings.feishu_step_verify")}
                    </div>
                    <div className={styles.row}>
                      <button
                        className={styles.hooks_install_btn}
                        onClick={handleConnectFeishu}
                        disabled={feishuAuthorizing || !feishuCanVerify}
                      >
                        {feishuAuthorizing ? t("settings.feishu_authorizing") : t("settings.feishu_connect")}
                      </button>
                    </div>
                    {!feishuCanVerify && (
                      <div className={styles.row}>
                        <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                          {t("settings.feishu_verify_needs_creds")}
                        </span>
                      </div>
                    )}
                  </>
                )}

                {feishuError && (
                  <div className={styles.row}>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-error, #d33)" }}>
                      {t("settings.feishu_failed")}: {feishuError}
                    </span>
                  </div>
                )}
              </div>
            )}

            {/* ── Mobile Access — merged into Account ── */}
            {activeTab === "account" && (
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

            {/* ── Alerts — unified: master mute → floating panel → system
                 notifications → sound/speech. Replaces the old separate
                 Notifications + Sound tabs and the floating-panel toggle that
                 used to live under Interaction. ── */}
            {activeTab === "alerts" && (
              <div className={styles.section}>
                {/* Master mute — the single switch that silences EVERYTHING.
                    Writes tts-muted, which both the front-end decision queue
                    and the Rust notification TTS honour. */}
                <div className={styles.section_title}>{t("settings.alerts_master")}</div>
                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.mute_all")}</span>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                      {t("settings.mute_all_desc")}
                    </span>
                  </div>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={ttsMuted}
                      onChange={(e) => handleTtsMutedChange(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>

                {/* Visual — floating decision panel */}
                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.alerts_visual")}</div>
                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.floating_decision_panel")}</span>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                      {t("settings.floating_decision_panel_desc")}
                    </span>
                  </div>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={floatingDecisionPanel}
                      onChange={(e) => handleToggleFloatingDecisionPanel(e.target.checked)}
                    />
                    <span className={styles.toggle_slider} />
                  </label>
                </div>

                {/* System notifications */}
                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.notification_mode")}</div>
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

            {/* ── Sound / speech — merged into Alerts ── */}
            {activeTab === "alerts" && (
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

            {/* ── Usage ── */}
            {activeTab === "usage" && (
              <div className={styles.section}>
                <div className={styles.section_title}>{t("settings.usage")}</div>
                <div className={styles.row_hint} style={{ marginBottom: 10 }}>
                  {t("settings.usage_desc")}
                </div>
                <UsageTrendPanel />
              </div>
            )}
          </div>
        </div>
      </div>
    </div>
  );
}
