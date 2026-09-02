import { invoke } from "@tauri-apps/api/core";
import { isPermissionGranted, requestPermission } from "@tauri-apps/plugin-notification";
import { useCallback, useEffect, useState } from "react";
import { useTranslation } from "react-i18next";
import { useConnectionStore, useDetailStore, useUIStore } from "../store";
import { useKeepAwake } from "../hooks/useKeepAwake";
import { isWebBuild } from "../hostEnv";
import {
  getItem,
  setItem,
  resolveFeature,
  getFeatureState,
  setFeatureState,
  resolveFeatureState,
  featureDefault,
  getModeSelection,
  setModeSelection,
  modeDefault,
  type FeatureState,
} from "../storage";
import { TriStateToggle } from "./TriStateToggle";
import { playChime, speakText, getVoices, CHIME_PRESETS, type ChimePreset, type TtsVoice } from "../audio";
import { AccountInfo } from "./AccountInfo";
import { EnvironmentPanel } from "./EnvironmentPanel";
import { SETTINGS_OPEN_TAB_KEY } from "../harnessErrors";
import { LanguageSwitcher } from "./LanguageSwitcher";
import { ThemeToggle } from "./ThemeToggle";
import { AgentSourceIcon } from "./SessionCard";
import { UsageTrendPanel } from "./UsageTrendPanel";
import styles from "./SettingsPanel.module.css";
import type { RemoteWorkspace, RemoteWorkspacesConfig } from "../types";
import { sshTargetOf, type HostHealth, type RemoteConnection } from "./ConnectionDialog";


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
  modelGuidanceInstalled: boolean;
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
  /** Same-tier model name in the other engine (e.g. "Luna" for "Haiku"). */
  alignedDisplay?: string;
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
  dailyReportPreference?: string | null;
}

type NotificationMode = "all" | "user_action" | "none";
type TtsMode = "chime_and_speech" | "chime_only" | "off";

// Redesigned IA: 3 everyday tabs (general / alerts / account) + 4 advanced
// tabs (interaction / model / integration / usage) shown under a collapsible
// "Advanced" group. See the settings-redesign plan.
type SettingsTab = "general" | "alerts" | "account" | "environment" | "interaction" | "model" | "integration" | "usage";
const BASE_TABS: SettingsTab[] = ["general", "alerts", "account"];
const ADVANCED_TABS: SettingsTab[] = ["environment", "interaction", "model", "integration", "usage"];

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
  // Environment = wrench: install/upgrade/login health for the agent harnesses.
  environment: (
    <svg viewBox="0 0 16 16" width="14" height="14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M9.5 3.5a3 3 0 0 1 3.9-.7L11 5.2l1.8 1.8 2.4-2.4a3 3 0 0 1-4.1 3.7L5.5 14a1.4 1.4 0 0 1-2-2l5.7-5.7a3 3 0 0 1 .3-2.8z" />
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
  // Integration = link (formerly Connection); covers hooks, binary, sources.
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

  // Cross-window tab deep-link (e.g. a spawn error's "open the environment
  // panel" button): consume SETTINGS_OPEN_TAB_KEY on mount, and via the
  // `storage` event when this window is already open. Consumed = removed, so
  // it never persists into an unrelated later open.
  useEffect(() => {
    const consume = () => {
      const tab = window.localStorage.getItem(SETTINGS_OPEN_TAB_KEY);
      if (!tab) return;
      window.localStorage.removeItem(SETTINGS_OPEN_TAB_KEY);
      if (([...BASE_TABS, ...ADVANCED_TABS] as string[]).includes(tab)) {
        setActiveTab(tab as SettingsTab);
        if ((ADVANCED_TABS as string[]).includes(tab)) setShowAdvanced(true);
      }
    };
    consume();
    window.addEventListener("storage", consume);
    return () => window.removeEventListener("storage", consume);
  }, []);

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

  // ── Remote hosts (rca executors) ─────────────────────────────────────────
  //
  // One list of hosts, each carrying its capabilities, with the workspaces
  // registered on it nested underneath. Previously this was a flat list of
  // workspace paths plus two parallel registration forms — and, because both
  // forms bound the same `rwPath` / `rwLabel` state, typing into one silently
  // filled the other.
  const [sshHosts, setSshHosts] = useState<RemoteConnection[]>([]);
  const [remoteWorkspaces, setRemoteWorkspaces] = useState<RemoteWorkspace[]>([]);
  const [rwError, setRwError] = useState("");
  // stdio-over-ssh auto-installer wizard. The ssh-target picker merges three
  // sources: ~/.ssh/config Host aliases, saved Fleet connections, and a manual
  // `user@host` entry — all resolved to a RemoteConnection for install_rca_remote.
  const [rwSavedConns, setRwSavedConns] = useState<RemoteConnection[]>([]);
  const [rwSshProfiles, setRwSshProfiles] = useState<string[]>([]);
  const [rwConnId, setRwConnId] = useState(""); // "saved:<id>" | "profile:<alias>" | "__manual__"
  const [rwManualTarget, setRwManualTarget] = useState(""); // user@host when __manual__
  const [rwInstalling, setRwInstalling] = useState(false);
  const [rwInstallSteps, setRwInstallSteps] = useState<string[]>([]);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    (async () => {
      const { listen } = await import("@tauri-apps/api/event");
      unlisten = await listen<{ step: string; done: boolean }>("rca-install-progress", (e) => {
        setRwInstallSteps((prev) => [...prev, e.payload.step]);
      });
    })();
    return () => unlisten?.();
  }, []);

  useEffect(() => {
    invoke<RemoteWorkspacesConfig>("list_remote_workspaces")
      .then((cfg) => setRemoteWorkspaces(cfg.workspaces ?? []))
      .catch(() => {});
    // The BACKEND host's book — that is the one a session resolves a
    // workspace's `hostId` against. Under a local backend it is the same
    // records `list_saved_connections` returns, which is what makes this one
    // merged list rather than two.
    invoke<RemoteConnection[]>("list_ssh_hosts")
      .then((hosts) => setSshHosts(hosts ?? []))
      .catch(() => {});
    // Deliberately still local: "which Fleet backends can THIS desktop dial".
    // Only feeds the add-a-host picker below.
    invoke<RemoteConnection[]>("list_saved_connections")
      .then((conns) => setRwSavedConns(conns ?? []))
      .catch(() => {});
    invoke<string[]>("list_ssh_profiles")
      .then((profiles) => setRwSshProfiles(profiles ?? []))
      .catch(() => {});
  }, []);

  // Resolve the picker selection to the RemoteConnection install_rca_remote needs.
  const resolveInstallConn = useCallback((): RemoteConnection | null => {
    if (rwConnId.startsWith("saved:")) {
      return rwSavedConns.find((c) => c.id === rwConnId.slice("saved:".length)) ?? null;
    }
    if (rwConnId.startsWith("profile:")) {
      const alias = rwConnId.slice("profile:".length);
      return {
        id: rwConnId, label: alias, host: "", port: 22, username: "",
        identityFile: null, jumpHost: null, sshProfile: alias,
      };
    }
    if (rwConnId === "__manual__") {
      const t = rwManualTarget.trim();
      const at = t.indexOf("@");
      if (at <= 0 || at === t.length - 1) return null; // require user@host
      return {
        id: "manual", label: t, host: t.slice(at + 1), port: 22, username: t.slice(0, at),
        identityFile: null, jumpHost: null, sshProfile: null,
      };
    }
    return null;
  }, [rwConnId, rwManualTarget, rwSavedConns]);

  // Provision the picked host as an rca executor. No workspace path: setting up
  // a host and choosing a directory on it are two decisions, and fusing them
  // used to make the hardest field (an absolute path that must exist
  // identically on both machines) a prerequisite for the easy one.
  const handleInstallRca = useCallback(async () => {
    const conn = resolveInstallConn();
    if (!conn) return;
    setRwInstalling(true);
    setRwError("");
    setRwInstallSteps([]);
    try {
      setSshHosts(await invoke<RemoteConnection[]>("install_rca_on_host", { conn }));
      setRwConnId("");
      setRwManualTarget("");
    } catch (e) {
      setRwError(String(e));
    } finally {
      setRwInstalling(false);
    }
  }, [resolveInstallConn]);

  const handleRemoveRemoteWorkspace = useCallback(async (path: string) => {
    setRwError("");
    try {
      const cfg = await invoke<RemoteWorkspacesConfig>("remove_remote_workspace", { path });
      setRemoteWorkspaces(cfg.workspaces ?? []);
    } catch (e) {
      setRwError(String(e));
    }
  }, []);

  // Removing a host that still has workspaces would leave them resolving a
  // `hostId` that is gone — which fails loudly at spawn, but only then. Say so
  // up front instead.
  const handleRemoveHost = useCallback(async (host: RemoteConnection) => {
    setRwError("");
    const orphans = remoteWorkspaces.filter((w) => w.hostId === host.id);
    if (orphans.length > 0) {
      setRwError(
        t("settings.remote_host_remove_blocked", {
          count: orphans.length,
          paths: orphans.map((w) => w.path).join(", "),
        }),
      );
      return;
    }
    try {
      setSshHosts(await invoke<RemoteConnection[]>("remove_ssh_host", { id: host.id }));
    } catch (e) {
      setRwError(String(e));
    }
  }, [remoteWorkspaces, t]);

  const [rwUpdating, setRwUpdating] = useState<string | null>(null);
  const handleUpdateRca = useCallback(async (path: string) => {
    setRwError("");
    setRwInstallSteps([]);
    setRwUpdating(path);
    try {
      const cfg = await invoke<RemoteWorkspacesConfig>("update_rca_remote", { path });
      setRemoteWorkspaces(cfg.workspaces ?? []);
      setSshHosts(await invoke<RemoteConnection[]>("list_ssh_hosts"));
    } catch (e) {
      setRwError(String(e));
    } finally {
      setRwUpdating(null);
    }
  }, []);

  // Whether a host is actually usable, on demand. Until this runs, a row can
  // only report what was true at install time — which is why a dead host used
  // to be discoverable only by starting a session and watching it fail.
  const [rwHealth, setRwHealth] = useState<Record<string, HostHealth | "probing">>({});
  const handleTestHost = useCallback(async (host: RemoteConnection) => {
    const target = sshTargetOf(host);
    if (!target) return;
    setRwHealth((prev) => ({ ...prev, [host.id]: "probing" }));
    try {
      const health = await invoke<HostHealth>("remote_host_health", { sshTarget: target });
      setRwHealth((prev) => ({ ...prev, [host.id]: health }));
    } catch (e) {
      setRwHealth((prev) => ({
        ...prev,
        [host.id]: { sshOk: false, stdioOk: false, error: String(e) },
      }));
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
      if (resolveFeature("guard-enabled") && !plan.guardInstalled) {
        invoke("apply_guard_hook").catch((e: unknown) =>
          console.error("auto-apply guard hook:", e),
        );
      }
      if (resolveFeature("elicitation-enabled") && !plan.elicitationInstalled) {
        invoke("apply_elicitation_hook").catch((e: unknown) =>
          console.error("auto-apply elicitation hook:", e),
        );
      }
      // These are default-ON: the localStorage checkbox is the source of truth
      // for the user's choice (absent → on), and disk is healed to match. Apply
      // is idempotent, so it installs the sentinel when missing and refreshes
      // title/locale when present. A stored "false" means the user explicitly
      // turned it off in Settings — respected here (no auto-apply). This mirrors
      // the guard/elicitation pattern above; we deliberately do NOT write the
      // key back from disk, which would strand the default-on the moment disk
      // showed not-yet-installed.
      if (resolveFeature("interaction-mode-enabled")) {
        invoke("apply_interaction_mode").catch((e: unknown) =>
          console.error("auto-apply interaction mode:", e),
        );
      }
      if (resolveFeature("plan-approval-enabled") && !plan.planApprovalInstalled) {
        invoke("apply_plan_approval_hook").catch((e: unknown) =>
          console.error("auto-apply plan approval:", e),
        );
      }
      if (resolveFeature("prd-mode-enabled")) {
        invoke("apply_prd_mode").catch((e: unknown) =>
          console.error("auto-apply prd mode:", e),
        );
      }
      if (resolveFeature("wiki-guidance-enabled")) {
        invoke("apply_wiki_guidance").catch((e: unknown) =>
          console.error("auto-apply wiki guidance:", e),
        );
      }
      if (resolveFeature("model-guidance-enabled")) {
        invoke("apply_model_guidance").catch((e: unknown) =>
          console.error("auto-apply model guidance:", e),
        );
      }
      // Codex guidance is derived, not a separate toggle: mirror the Claude
      // concept toggles (interaction / PRD / wiki / model) onto
      // ~/.codex/AGENTS.md on startup. reconcile reads the Claude sentinels
      // (disk source of truth), refreshes title/locale, and migrates away any
      // legacy monolithic codex-guidance block from before the split.
      invoke("reconcile_codex_guidance").catch((e: unknown) =>
        console.error("startup reconcile codex guidance:", e),
      );
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
  const [guardState, setGuardState] = useState<FeatureState>(
    () => getFeatureState("guard-enabled"),
  );
  const guardEnabled = resolveFeatureState(guardState, "guard-enabled");
  const [guardLlmState, setGuardLlmState] = useState<FeatureState>(
    () => getFeatureState("guard-llm-analysis"),
  );

  const handleToggleGuard = useCallback(async (state: FeatureState) => {
    setGuardState(state);
    setFeatureState("guard-enabled", state);
    const enabled = resolveFeatureState(state, "guard-enabled");
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

  const handleToggleGuardLlm = useCallback((state: FeatureState) => {
    setGuardLlmState(state);
    setFeatureState("guard-llm-analysis", state);
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
  const [elicitationState, setElicitationState] = useState<FeatureState>(
    () => getFeatureState("elicitation-enabled"),
  );
  const elicitationEnabled = resolveFeatureState(elicitationState, "elicitation-enabled");

  // ── Interaction mode state (default on) ───────────────────────────────
  const [interactionModeState, setInteractionModeState] = useState<FeatureState>(
    () => getFeatureState("interaction-mode-enabled"),
  );

  // Mirror the Claude-side concept toggles onto codex's ~/.codex/AGENTS.md.
  // A single concept toggle drives BOTH carriers: after the Claude apply/remove
  // lands, this reads the resulting Claude sentinel state and reconciles the
  // matching codex blocks. Idempotent and order-independent, so it's safe to
  // call after every concept toggle and on startup.
  const reconcileCodexGuidance = useCallback(() => {
    return invoke("reconcile_codex_guidance").catch((e: unknown) =>
      console.error("reconcile codex guidance:", e),
    );
  }, []);

  const handleToggleInteractionMode = useCallback(async (state: FeatureState) => {
    setInteractionModeState(state);
    setFeatureState("interaction-mode-enabled", state);
    const enabled = resolveFeatureState(state, "interaction-mode-enabled");
    try {
      if (enabled) {
        await invoke("apply_interaction_mode");
      } else {
        await invoke("remove_interaction_mode");
      }
      await reconcileCodexGuidance();
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("interaction mode toggle failed:", e);
    }
  }, [reconcileCodexGuidance]);

  // ── PRD discipline mode state (default on) ────────────────────────────
  const [prdModeState, setPrdModeState] = useState<FeatureState>(
    () => getFeatureState("prd-mode-enabled"),
  );

  const handleTogglePrdMode = useCallback(async (state: FeatureState) => {
    setPrdModeState(state);
    setFeatureState("prd-mode-enabled", state);
    const enabled = resolveFeatureState(state, "prd-mode-enabled");
    try {
      if (enabled) {
        await invoke("apply_prd_mode");
      } else {
        await invoke("remove_prd_mode");
      }
      await reconcileCodexGuidance();
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("prd mode toggle failed:", e);
    }
  }, [reconcileCodexGuidance]);

  // ── Wiki guidance state (default on) ───────────────────────────────────
  const [wikiGuidanceState, setWikiGuidanceState] = useState<FeatureState>(
    () => getFeatureState("wiki-guidance-enabled"),
  );

  const handleToggleWikiGuidance = useCallback(async (state: FeatureState) => {
    setWikiGuidanceState(state);
    setFeatureState("wiki-guidance-enabled", state);
    const enabled = resolveFeatureState(state, "wiki-guidance-enabled");
    try {
      if (enabled) {
        await invoke("apply_wiki_guidance");
      } else {
        await invoke("remove_wiki_guidance");
      }
      await reconcileCodexGuidance();
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("wiki guidance toggle failed:", e);
    }
  }, [reconcileCodexGuidance]);

  // ── Model guidance state (default on) ──────────────────────────────────
  const [modelGuidanceState, setModelGuidanceState] = useState<FeatureState>(
    () => getFeatureState("model-guidance-enabled"),
  );

  const handleToggleModelGuidance = useCallback(async (state: FeatureState) => {
    setModelGuidanceState(state);
    setFeatureState("model-guidance-enabled", state);
    const enabled = resolveFeatureState(state, "model-guidance-enabled");
    try {
      if (enabled) {
        await invoke("apply_model_guidance");
      } else {
        await invoke("remove_model_guidance");
      }
      await reconcileCodexGuidance();
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("model guidance toggle failed:", e);
    }
  }, [reconcileCodexGuidance]);

  const handleToggleElicitation = useCallback(async (state: FeatureState) => {
    setElicitationState(state);
    setFeatureState("elicitation-enabled", state);
    const enabled = resolveFeatureState(state, "elicitation-enabled");
    try {
      if (enabled) {
        await invoke("apply_elicitation_hook");
      } else {
        await invoke("remove_elicitation_hook");
        // Interaction mode depends on elicitation; disable it together — and
        // mirror the removal onto codex so its interaction block goes too. Force
        // an explicit "off" (not "default") so it can't silently follow a
        // default-on back to enabled while elicitation is gone.
        if (resolveFeature("interaction-mode-enabled")) {
          setInteractionModeState("off");
          setFeatureState("interaction-mode-enabled", "off");
          await invoke("remove_interaction_mode").catch(() => {});
          await reconcileCodexGuidance();
        }
      }
      invoke<HookSetupPlan>("get_hooks_setup_plan").then(setHooksPlan).catch(() => {});
    } catch (e) {
      console.error("elicitation hook toggle failed:", e);
    }
  }, [reconcileCodexGuidance]);

  // ── Plan approval state (default on) ──────────────────────────────────
  const [planApprovalState, setPlanApprovalState] = useState<FeatureState>(
    () => getFeatureState("plan-approval-enabled"),
  );

  const handleTogglePlanApproval = useCallback(async (state: FeatureState) => {
    setPlanApprovalState(state);
    setFeatureState("plan-approval-enabled", state);
    const enabled = resolveFeatureState(state, "plan-approval-enabled");
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

  // ── Notifications state (tristate: concrete mode or "default") ──────────
  const [notifSelection, setNotifSelection] = useState<NotificationMode | "default">(
    () => getModeSelection("notification-mode") as NotificationMode | "default",
  );
  const notifMode: NotificationMode =
    notifSelection === "default"
      ? (modeDefault("notification-mode") as NotificationMode)
      : notifSelection;
  const [notifPermission, setNotifPermission] = useState<boolean | null>(null);

  useEffect(() => {
    isPermissionGranted().then(setNotifPermission).catch(() => {});
  }, []);

  const handleNotifModeChange = useCallback((sel: NotificationMode | "default") => {
    setNotifSelection(sel);
    setModeSelection("notification-mode", sel);
    const effective =
      sel === "default" ? (modeDefault("notification-mode") as NotificationMode) : sel;
    invoke("set_notification_mode", { mode: effective }).catch(() => {});
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

  // ── TTS state (tristate: concrete mode or "default") ────────────────────
  const [ttsSelection, setTtsSelection] = useState<TtsMode | "default">(
    () => getModeSelection("tts-mode") as TtsMode | "default",
  );
  const ttsMode: TtsMode =
    ttsSelection === "default" ? (modeDefault("tts-mode") as TtsMode) : ttsSelection;

  const handleTtsModeChange = useCallback((sel: TtsMode | "default") => {
    setTtsSelection(sel);
    setModeSelection("tts-mode", sel);
  }, []);

  // ── Master mute ───────────────────────────────────────────────────────
  // `tts-muted` is read by both the front-end decision-panel queue
  // (audio.ts playDecisionAlert/playAlertSound) and the Rust notification
  // TTS path (gui.rs play_tts_for_notification). It used to have NO settings
  // UI — only the Lite-mode top-bar button — which is why "I turned sound
  // off but the decision panel still spoke" happened. Surface it here as the
  // single master switch.
  const [ttsMutedState, setTtsMutedState] = useState<FeatureState>(
    () => getFeatureState("tts-muted"),
  );

  const handleTtsMutedChange = useCallback((state: FeatureState) => {
    setTtsMutedState(state);
    setFeatureState("tts-muted", state);
  }, []);

  // ── Chime preset state (tristate: concrete preset or "default") ─────────
  const [chimeSelection, setChimeSelection] = useState<ChimePreset | "default">(
    () => getModeSelection("chime-sound") as ChimePreset | "default",
  );
  const chimePreset: ChimePreset =
    chimeSelection === "default" ? (modeDefault("chime-sound") as ChimePreset) : chimeSelection;

  const handleChimeChange = useCallback((sel: ChimePreset | "default") => {
    setChimeSelection(sel);
    setModeSelection("chime-sound", sel);
    const effective =
      sel === "default" ? (modeDefault("chime-sound") as ChimePreset) : sel;
    playChime(effective);
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
  const [personalizedMascotState, setPersonalizedMascotState] = useState<FeatureState>(
    () => getFeatureState("personalized-mascot"),
  );

  const handleTogglePersonalizedMascot = useCallback((state: FeatureState) => {
    setPersonalizedMascotState(state);
    setFeatureState("personalized-mascot", state);
  }, []);

  // ── Mascot visibility state ────────────────────────────────────────────
  const [mascotVisibleState, setMascotVisibleState] = useState<FeatureState>(
    () => getFeatureState("mascot-visible"),
  );
  const mascotVisible = resolveFeatureState(mascotVisibleState, "mascot-visible");

  const handleToggleMascotVisible = useCallback(async (state: FeatureState) => {
    setMascotVisibleState(state);
    setFeatureState("mascot-visible", state);
    const enabled = resolveFeatureState(state, "mascot-visible");
    const { emit } = await import("@tauri-apps/api/event");
    await emit("overlay-mascot-visible-changed", enabled).catch(() => {});
  }, []);

  // ── Floating decision panel state ──────────────────────────────────────
  const [floatingDecisionPanelState, setFloatingDecisionPanelState] = useState<FeatureState>(
    () => getFeatureState("floating-decision-panel"),
  );

  const handleToggleFloatingDecisionPanel = useCallback(async (state: FeatureState) => {
    setFloatingDecisionPanelState(state);
    setFeatureState("floating-decision-panel", state);
    const enabled = resolveFeatureState(state, "floating-decision-panel");
    const { emit } = await import("@tauri-apps/api/event");
    await emit("overlay-floating-decision-panel-changed", enabled).catch(() => {});
  }, []);

  // ── Auto update check state ────────────────────────────────────────────
  const [autoUpdateCheckState, setAutoUpdateCheckState] = useState<FeatureState>(
    () => getFeatureState("auto-update-check"),
  );

  const handleToggleAutoUpdateCheck = useCallback((state: FeatureState) => {
    setAutoUpdateCheckState(state);
    setFeatureState("auto-update-check", state);
  }, []);

  // ── Group handoff-relay sessions ───────────────────────────────────────────
  // Lives in the UI store (which persists it) so the task list reacts live when
  // this is flipped, rather than only after a restart.
  const groupHandoff = useUIStore((s) => s.historyGroupHandoff);
  const setGroupHandoff = useUIStore((s) => s.setHistoryGroupHandoff);

  // ── LLM provider state ──────────────────────────────────────────────────
  const [llmProviders, setLlmProviders] = useState<LlmProviderInfo[]>([]);
  const [llmConfig, setLlmConfigState] = useState<LlmConfig>(() => ({
    provider: getItem("llm-provider") || "claude",
    fastModel: getItem("llm-model-fast") || "haiku",
    standardModel: getItem("llm-model-standard") || "sonnet",
    dailyReportPreference: getItem("llm-daily-report-preference") || "claude",
  }));

  useEffect(() => {
    invoke<LlmProviderInfo[]>("list_llm_providers").then(setLlmProviders).catch(() => {});
    invoke<LlmConfig>("get_llm_config").then((cfg) => {
      setLlmConfigState(cfg);
      setItem("llm-provider", cfg.provider);
      setItem("llm-model-fast", cfg.fastModel);
      setItem("llm-model-standard", cfg.standardModel);
      setItem("llm-daily-report-preference", cfg.dailyReportPreference || cfg.provider || "claude");
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
      setItem("llm-daily-report-preference", next.dailyReportPreference || next.provider || "claude");
      invoke("set_llm_config", { config: next }).catch(() => {});
      return next;
    });
  }, [llmProviders]);

  const currentProviderInfo = llmProviders.find((p) => p.name === llmConfig.provider);
  const dualReportProvidersEnabled = ["claude-code", "codex"].every((name) =>
    sources.some((source) => source.name === name && source.enabled && source.available),
  );

  // Show the cross-engine tier sibling (e.g. "Haiku / Luna") only when both
  // engines are active, since that's the only time quota fallback swaps them.
  const modelOptionLabel = useCallback((m: LlmModel) => (
    dualReportProvidersEnabled && m.alignedDisplay
      ? `${m.displayName} / ${m.alignedDisplay}`
      : m.displayName
  ), [dualReportProvidersEnabled]);

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
    environment: t("settings.tab_environment"),
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
                  <TriStateToggle
                    value={autoUpdateCheckState}
                    defaultOn={featureDefault("auto-update-check")}
                    onChange={handleToggleAutoUpdateCheck}
                  />
                </div>

                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.group_handoff")}</span>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                      {t("settings.group_handoff_desc")}
                    </span>
                  </div>
                  <label className={styles.toggle}>
                    <input
                      type="checkbox"
                      checked={groupHandoff}
                      onChange={(e) => setGroupHandoff(e.target.checked)}
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
                {llmConfig.provider !== "none" && dualReportProvidersEnabled && (
                  <div className={styles.row}>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                      {t("settings.llm_provider_routing_desc")}
                    </span>
                  </div>
                )}
                {llmConfig.provider === "none" && (
                  <div className={styles.row}>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-warning, #e8a838)" }}>
                      {t("settings.llm_disabled_warning")}
                    </span>
                  </div>
                )}
                {llmConfig.provider !== "none" && dualReportProvidersEnabled && (
                  <div className={styles.row}>
                    <div>
                      <span className={styles.row_label}>{t("settings.llm_daily_report_preference")}</span>
                      <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                        {t("settings.llm_daily_report_preference_desc")}
                      </span>
                    </div>
                    <select className={styles.select} style={{ flex: "none", width: 180 }}
                      value={llmConfig.dailyReportPreference || llmConfig.provider || "claude"}
                      onChange={(e) => handleLlmConfigChange({ dailyReportPreference: e.target.value })}>
                      <option value="claude">Claude Code</option>
                      <option value="codex">Codex</option>
                    </select>
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
                          <option key={m.id} value={m.id}>{modelOptionLabel(m)}</option>
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
                          <option key={m.id} value={m.id}>{modelOptionLabel(m)}</option>
                        ))}
                      </select>
                    </div>
                  </>
                )}
                {llmConfig.provider !== "none" && dualReportProvidersEnabled && (
                  <div className={styles.row}>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                      {t("settings.llm_model_alignment_desc")}
                    </span>
                  </div>
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
                  <TriStateToggle
                    value={mascotVisibleState}
                    defaultOn={featureDefault("mascot-visible")}
                    onChange={handleToggleMascotVisible}
                  />
                </div>
                {mascotVisible && (
                  <div className={styles.row}>
                    <div>
                      <span className={styles.row_label}>{t("settings.personalized_mascot")}</span>
                      <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                        {t("settings.personalized_mascot_desc")}
                      </span>
                    </div>
                    <TriStateToggle
                      value={personalizedMascotState}
                      defaultOn={featureDefault("personalized-mascot")}
                      onChange={handleTogglePersonalizedMascot}
                    />
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

            {/* ── Environment (advanced): harness install/upgrade/login health ── */}
            {activeTab === "environment" && (
              <div className={styles.section}>
                <EnvironmentPanel />
              </div>
            )}

            {/* ── Integration (advanced): hooks + Claude binary + sources ── */}
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

                {/* rca remote hosts: still debug-only until the flow is
                    finished (import.meta.env.DEV is false in `vite build`).
                    The composer-side "pick a host, browse it, register" flow
                    lands next; until it does, this section can set a host up
                    but not choose a directory on it. */}
                {import.meta.env.DEV && (
                  <>
                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.remote_hosts")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.remote_hosts_desc")}
                  </span>
                </div>
                {sshHosts.length === 0 && (
                  <div className={styles.row}>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                      {t("settings.remote_host_empty")}
                    </span>
                  </div>
                )}
                {sshHosts.map((h) => {
                  const target = sshTargetOf(h);
                  const health = rwHealth[h.id];
                  const spaces = remoteWorkspaces.filter((w) => w.hostId === h.id);
                  return (
                    <div key={h.id} style={{ borderTop: "1px solid var(--color-border)", paddingTop: 6, marginTop: 6 }}>
                      <div className={styles.row}>
                        <span className={styles.row_label} style={{ minWidth: 0 }}>
                          {h.label || target}
                          <span style={{ display: "block", fontSize: 10, color: "var(--color-text-dim)", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap", maxWidth: 360, fontFamily: "var(--font-mono, monospace)" }}>
                            {target}
                          </span>
                          <span style={{ display: "block", fontSize: 10, marginTop: 2 }}>
                            {h.rcaPath
                              ? <span style={{ color: "var(--color-text-dim)" }}>{t("settings.remote_host_cap_rca")}</span>
                              : <span style={{ color: "var(--color-text-dim)" }}>{t("settings.remote_host_cap_none")}</span>}
                            {health === "probing" && <span style={{ marginLeft: 8 }}>{t("settings.remote_host_testing")}</span>}
                            {health && health !== "probing" && (
                              <span style={{ marginLeft: 8, color: health.sshOk && health.stdioOk ? "var(--color-ok, inherit)" : "var(--color-danger, inherit)" }}>
                                {health.sshOk && health.stdioOk
                                  ? t("settings.remote_host_ready", { version: health.rcaVersion ?? "rca" })
                                  : (health.error ?? t("settings.remote_host_unreachable"))}
                              </span>
                            )}
                          </span>
                        </span>
                        <button
                          className={styles.sources_restart_btn}
                          onClick={() => handleTestHost(h)}
                          disabled={health === "probing" || !target}
                        >
                          {t("settings.remote_host_test")}
                        </button>
                        <button
                          className={styles.sources_restart_btn}
                          onClick={() => handleRemoveHost(h)}
                        >
                          {t("settings.remote_ws_remove")}
                        </button>
                      </div>
                      {spaces.map((w) => (
                        <div className={styles.row} key={w.path} style={{ paddingLeft: 14 }}>
                          <span className={styles.row_label} style={{ minWidth: 0, fontSize: 11, fontFamily: "var(--font-mono, monospace)" }}>
                            {w.path}
                          </span>
                          <button
                            className={styles.sources_restart_btn}
                            onClick={() => handleUpdateRca(w.path)}
                            disabled={rwUpdating !== null}
                          >
                            {rwUpdating === w.path
                              ? t("settings.remote_ws_installing")
                              : t("settings.remote_ws_update_btn")}
                          </button>
                          <button
                            className={styles.sources_restart_btn}
                            onClick={() => handleRemoveRemoteWorkspace(w.path)}
                          >
                            {t("settings.remote_ws_remove")}
                          </button>
                        </div>
                      ))}
                    </div>
                  );
                })}

                {/* Workspaces whose entry predates the host book: they carry a
                    baked-in ssh target rather than a host id, so no row above
                    owns them. Shown so they are still removable. */}
                {remoteWorkspaces.filter((w) => !w.hostId).length > 0 && (
                  <>
                    <div className={styles.section_title} style={{ marginTop: 12 }}>
                      {t("settings.remote_ws_unattached_title")}
                    </div>
                    {remoteWorkspaces.filter((w) => !w.hostId).map((w) => (
                      <div className={styles.row} key={w.path}>
                        <span className={styles.row_label} style={{ minWidth: 0 }}>
                          <span style={{ fontFamily: "var(--font-mono, monospace)", fontSize: 11 }}>{w.path}</span>
                          <span style={{ display: "block", fontSize: 10, color: "var(--color-text-dim)" }}>
                            {w.sshTarget ?? w.pairingCode}
                          </span>
                        </span>
                        {w.sshTarget && (
                          <button
                            className={styles.sources_restart_btn}
                            onClick={() => handleUpdateRca(w.path)}
                            disabled={rwUpdating !== null}
                          >
                            {rwUpdating === w.path
                              ? t("settings.remote_ws_installing")
                              : t("settings.remote_ws_update_btn")}
                          </button>
                        )}
                        <button
                          className={styles.sources_restart_btn}
                          onClick={() => handleRemoveRemoteWorkspace(w.path)}
                        >
                          {t("settings.remote_ws_remove")}
                        </button>
                      </div>
                    ))}
                  </>
                )}

                {/* Add a host: pick an ssh target, install rca. No workspace
                    path — that is the composer's job. */}
                <div className={styles.section_title} style={{ marginTop: 12 }}>
                  {t("settings.remote_host_add_title")}
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.remote_host_add_desc")}
                  </span>
                </div>
                <div className={styles.row} style={{ flexWrap: "wrap", gap: 6 }}>
                  <select
                    className={styles.select}
                    style={{ flex: "1 1 180px" }}
                    value={rwConnId}
                    onChange={(e) => setRwConnId(e.target.value)}
                  >
                    <option value="">{t("settings.remote_ws_pick_conn")}</option>
                    {rwSshProfiles.length > 0 && (
                      <optgroup label={t("settings.remote_ws_src_ssh_config")}>
                        {rwSshProfiles.map((alias) => (
                          <option key={`profile:${alias}`} value={`profile:${alias}`}>{alias}</option>
                        ))}
                      </optgroup>
                    )}
                    {rwSavedConns.length > 0 && (
                      <optgroup label={t("settings.remote_ws_src_saved")}>
                        {rwSavedConns.map((c) => (
                          <option key={`saved:${c.id}`} value={`saved:${c.id}`}>
                            {c.label || c.sshProfile || `${c.username}@${c.host}`}
                          </option>
                        ))}
                      </optgroup>
                    )}
                    <option value="__manual__">{t("settings.remote_ws_src_manual")}</option>
                  </select>
                  {rwConnId === "__manual__" && (
                    <input
                      className={styles.select}
                      style={{ flex: "1 1 160px" }}
                      value={rwManualTarget}
                      onChange={(e) => setRwManualTarget(e.target.value)}
                      placeholder="user@host"
                      spellCheck={false}
                    />
                  )}
                  <button
                    className={styles.sources_restart_btn}
                    onClick={handleInstallRca}
                    disabled={rwInstalling || !resolveInstallConn()}
                  >
                    {rwInstalling
                      ? t("settings.remote_ws_installing")
                      : t("settings.remote_host_install_btn")}
                  </button>
                </div>
                {rwSavedConns.length === 0 && rwSshProfiles.length === 0 && (
                  <div className={styles.row}>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                      {t("settings.remote_ws_no_conns")}
                    </span>
                  </div>
                )}
                {rwInstallSteps.length > 0 && (
                  <div className={styles.row} style={{ flexDirection: "column", alignItems: "flex-start", gap: 2 }}>
                    {rwInstallSteps.map((s, i) => (
                      <span
                        key={i}
                        style={{ fontSize: 11, color: "var(--color-text-dim)", fontFamily: "var(--font-mono, monospace)" }}
                      >
                        {i === rwInstallSteps.length - 1 && rwInstalling ? "▸ " : "✓ "}
                        {s}
                      </span>
                    ))}
                  </div>
                )}
                {rwError && (
                  <p className={styles.hooks_error}>{rwError}</p>
                )}
                  </>
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
                  <TriStateToggle
                    value={elicitationState}
                    defaultOn={featureDefault("elicitation-enabled")}
                    onChange={handleToggleElicitation}
                  />
                </div>

                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.plan_approval")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.plan_approval_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.plan_approval_enabled")}</span>
                  <TriStateToggle
                    value={planApprovalState}
                    defaultOn={featureDefault("plan-approval-enabled")}
                    onChange={handleTogglePlanApproval}
                  />
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
                  <TriStateToggle
                    value={interactionModeState}
                    defaultOn={featureDefault("interaction-mode-enabled")}
                    disabled={!elicitationEnabled}
                    onChange={handleToggleInteractionMode}
                  />
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
                  <TriStateToggle
                    value={guardState}
                    defaultOn={featureDefault("guard-enabled")}
                    onChange={handleToggleGuard}
                  />
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.guard_llm_analysis")}</span>
                  <TriStateToggle
                    value={guardLlmState}
                    defaultOn={featureDefault("guard-llm-analysis")}
                    disabled={!guardEnabled}
                    onChange={handleToggleGuardLlm}
                  />
                </div>

                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.permissions_bypass")}</div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.permissions_bypass_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <div>
                    <span className={styles.row_label}>{t("settings.permissions_bypass_enabled")}</span>
                    <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                      {t("settings.permissions_bypass_recommended")}
                    </span>
                  </div>
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
                  <TriStateToggle
                    value={prdModeState}
                    defaultOn={featureDefault("prd-mode-enabled")}
                    onChange={handleTogglePrdMode}
                  />
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.wiki_guidance_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.wiki_guidance_enabled")}</span>
                  <TriStateToggle
                    value={wikiGuidanceState}
                    defaultOn={featureDefault("wiki-guidance-enabled")}
                    onChange={handleToggleWikiGuidance}
                  />
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.model_guidance_desc")}
                  </span>
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label}>{t("settings.model_guidance_enabled")}</span>
                  <TriStateToggle
                    value={modelGuidanceState}
                    defaultOn={featureDefault("model-guidance-enabled")}
                    onChange={handleToggleModelGuidance}
                  />
                </div>
                <div className={styles.row}>
                  <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)" }}>
                    {t("settings.codex_mirror_note")}
                  </span>
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
                              background: "var(--color-bg-field)",
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
                              background: "var(--color-bg-field)",
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
                              background: "var(--color-bg-field)",
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
                  <TriStateToggle
                    value={ttsMutedState}
                    defaultOn={featureDefault("tts-muted")}
                    onChange={handleTtsMutedChange}
                  />
                </div>

                {/* Visual — floating decision panel. The standalone window is a
                    desktop-host feature: a tab cannot open one, and
                    `show_decision_float` answers null in the browser build. The
                    toggle used to still be here and still be persisted, so
                    flipping it on in a tab silently sent every card to a window
                    that does not exist (see decisionSurface.ts). */}
                {!isWebBuild() && (
                  <>
                    <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.alerts_visual")}</div>
                    <div className={styles.row}>
                      <div>
                        <span className={styles.row_label}>{t("settings.floating_decision_panel")}</span>
                        <span className={styles.row_label} style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}>
                          {t("settings.floating_decision_panel_desc")}
                        </span>
                      </div>
                      <TriStateToggle
                        value={floatingDecisionPanelState}
                        defaultOn={featureDefault("floating-decision-panel")}
                        onChange={handleToggleFloatingDecisionPanel}
                      />
                    </div>
                  </>
                )}

                {/* System notifications.
                    Every part of this is desktop-only, and the browser build
                    used to render all of it anyway:
                      - the mode radios are pushed to the host with
                        `set_notification_mode`, a no-op in a tab, and nothing
                        on this side reads the stored value;
                      - the permission row asks the Tauri notification plugin,
                        whose browser stand-ins answer "not granted" then
                        "denied", so it permanently read 「已关闭」 behind a
                        button that could only reach another no-op
                        (`open_notification_settings`).
                    And there is no sender to enable in the first place: OS
                    notifications come from Rust (`send_os_notification`), which
                    fires on the machine the desktop app runs on — for a tab
                    pointed at fleet-cloud that is a container, not the user's
                    Mac. Wiring the browser's own `Notification` API would be a
                    feature, not a gate; until it exists, say so. */}
                {isWebBuild() ? (
                  <>
                    <div className={styles.section_title} style={{ marginTop: 18 }}>
                      {t("settings.notification_mode")}
                    </div>
                    <span
                      className={styles.row_label}
                      style={{ fontSize: 11, color: "var(--color-text-dim)", display: "block", marginTop: 2 }}
                    >
                      {t("settings.notification_web_unavailable")}
                    </span>
                  </>
                ) : (
                <>
                <div className={styles.section_title} style={{ marginTop: 18 }}>{t("settings.notification_mode")}</div>
                {(["all", "user_action", "none"] as const).map((mode) => (
                  <label className={styles.radio_row} key={mode}>
                    <input
                      type="radio"
                      name="notif-mode"
                      checked={notifSelection === mode}
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
                <label className={styles.radio_row} key="default">
                  <input
                    type="radio"
                    name="notif-mode"
                    checked={notifSelection === "default"}
                    onChange={() => handleNotifModeChange("default")}
                    className={styles.radio_input}
                  />
                  <div className={styles.radio_label}>
                    <span className={styles.radio_title}>{t("settings.mode_default_title")}</span>
                    <span className={styles.radio_desc}>
                      {t("settings.mode_default_desc", {
                        value: t(`settings.notify_${modeDefault("notification-mode")}`),
                      })}
                    </span>
                  </div>
                </label>

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
                </>
                )}
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
                      checked={ttsSelection === mode}
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
                <label className={styles.radio_row} key="default">
                  <input
                    type="radio"
                    name="tts-mode"
                    checked={ttsSelection === "default"}
                    onChange={() => handleTtsModeChange("default")}
                    className={styles.radio_input}
                  />
                  <div className={styles.radio_label}>
                    <span className={styles.radio_title}>{t("settings.mode_default_title")}</span>
                    <span className={styles.radio_desc}>
                      {t("settings.mode_default_desc", {
                        value: t(`settings.tts_${modeDefault("tts-mode")}`),
                      })}
                    </span>
                  </div>
                </label>

                {ttsMode !== "off" && (
                  <>
                    <div className={styles.section_title} style={{ marginTop: 18 }}>
                      {t("settings.chime_sound")}
                    </div>
                    <div className={styles.row}>
                      <select
                        className={styles.select}
                        value={chimeSelection}
                        onChange={(e) => handleChimeChange(e.target.value as ChimePreset | "default")}
                      >
                        <option value="default">
                          {t("settings.mode_default_option", {
                            value: t(`settings.chime_${modeDefault("chime-sound")}`),
                          })}
                        </option>
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
