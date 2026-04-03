/**
 * Mock layer using Tauri's official mocks API.
 * Allows the app to run in a plain browser without the Tauri runtime.
 *
 * Activated by importing and calling installMocks() before any app code.
 */

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { SessionInfo } from "../types";
import {
  MOCK_SESSIONS,
  MOCK_ACCOUNT_INFO,
  MOCK_CURSOR_USAGE,
  MOCK_CODEX_USAGE,
  MOCK_OPENCLAW_USAGE,
  MOCK_OPENCLAW_ACCOUNT,
  MOCK_MEMORIES,
  MOCK_MEMORY_CONTENT,
  MOCK_MEMORY_HISTORY,
  MOCK_SOURCES_CONFIG,
  MOCK_SETUP_STATUS,
  MOCK_HOOKS_PLAN,
  MOCK_DETECTED_TOOLS,
  MOCK_WAITING_ALERTS,
  MOCK_SKILL_HISTORY,
  MOCK_AUDIT_SUMMARY,
  MOCK_DAILY_REPORT,
  MOCK_HEATMAP_STATS,
  MOCK_LESSONS,
  MOCK_TIMELINE_REPORTS,
  getMessagesForSession,
} from "./data";

// ── Dynamic session state (simulates live updates) ──────────────────────────

let currentSessions: SessionInfo[] = structuredClone(MOCK_SESSIONS);

/** Nudge token counts and speeds to simulate live activity */
function tickSessions() {
  currentSessions = currentSessions.map((s) => {
    if (s.status === "idle") return s;
    const jitter = (Math.random() - 0.3) * 5;
    const newSpeed = Math.max(0, s.tokenSpeed + jitter);
    const tokensAdded = Math.round(newSpeed * 2);
    return {
      ...s,
      tokenSpeed: Math.round(newSpeed * 10) / 10,
      totalOutputTokens: s.totalOutputTokens + tokensAdded,
      lastActivityMs: Date.now() - Math.random() * 5000,
    };
  });
  // Push update via Tauri event system
  emit("sessions-updated", currentSessions);
}

// ── IPC handler ─────────────────────────────────────────────────────────────

function handleIPC(cmd: string, args: Record<string, unknown> = {}): unknown {
  switch (cmd) {
    case "list_sessions":
      return currentSessions;
    case "get_messages": {
      const jsonlPath = args.jsonlPath as string;
      const session = currentSessions.find((s) => s.jsonlPath === jsonlPath);
      return getMessagesForSession(session?.id ?? "");
    }
    case "start_watching_session":
    case "stop_watching_session":
    case "set_locale":
    case "disconnect_remote":
    case "set_source_enabled":
    case "apply_hooks_setup":
    case "kill_session":
    case "kill_workspace_sessions":
    case "delete_connection":
    case "connect_remote":
      return null;

    case "list_skills":
      return [];
    case "get_platform":
      return "macos";
    case "get_waiting_alerts":
      return MOCK_WAITING_ALERTS;
    case "get_account_info":
      return MOCK_ACCOUNT_INFO;
    case "get_source_account": {
      const source = args.source as string;
      if (source === "openclaw") return MOCK_OPENCLAW_ACCOUNT;
      if (source === "cursor") return MOCK_CURSOR_USAGE;
      return null;
    }
    case "get_source_usage": {
      const source = args.source as string;
      if (source === "cursor") return MOCK_CURSOR_USAGE;
      if (source === "codex") return MOCK_CODEX_USAGE;
      if (source === "openclaw") return MOCK_OPENCLAW_USAGE;
      return null;
    }
    case "list_memories":
      return MOCK_MEMORIES;
    case "get_memory_content":
      return MOCK_MEMORY_CONTENT;
    case "get_memory_history":
      return MOCK_MEMORY_HISTORY;
    case "get_sources_config":
      return MOCK_SOURCES_CONFIG;
    case "check_setup_status":
      return MOCK_SETUP_STATUS;
    case "get_hooks_setup_plan":
      return MOCK_HOOKS_PLAN;
    case "restart_app":
      window.location.reload();
      return null;
    case "get_skill_history": {
      const jp = args.jsonlPath as string;
      const sess = currentSessions.find((s) => s.jsonlPath === jp);
      return MOCK_SKILL_HISTORY[sess?.id ?? ""] ?? [];
    }
    case "get_audit_events":
      return MOCK_AUDIT_SUMMARY;
    case "detect_ai_tools":
      return MOCK_DETECTED_TOOLS;
    case "get_log_path":
      return "/tmp/claw-fleet.log";
    case "list_saved_connections":
      return [];
    case "list_ssh_profiles":
      return ["personal-server", "work-devbox", "staging-bastion"];
    case "pick_file":
      return null;
    case "install_fleet_cli":
      return "/usr/local/bin/fleet";
    case "save_skill_file":
      return "/Users/demo/.claude/skills/fleet.md";
    case "install_fleet_skill":
      return { success: true, path: "/Users/demo/.claude/skills/fleet.md" };
    // ── Daily Report ──
    case "get_daily_report": {
      const date = args.date as string;
      return MOCK_TIMELINE_REPORTS.get(date) ?? null;
    }
    case "list_daily_report_stats":
      return MOCK_HEATMAP_STATS;
    case "generate_daily_report":
      return MOCK_DAILY_REPORT;
    case "generate_daily_report_ai_summary":
      return MOCK_DAILY_REPORT.aiSummary;
    case "generate_daily_report_lessons":
      return MOCK_LESSONS;
    case "append_lesson_to_claude_md":
      return null;

    case "generate_mascot_quips":
      return {
        busy: [
          "All agents are running smoothly!",
          "Token throughput looking great today.",
        ],
        idle: [
          "Your fleet is in good shape, captain!",
          "Nice work on that last task!",
        ],
      };

    // Window plugin
    case "plugin:window|set_theme":
    case "plugin:window|set_title":
      return null;

    // Store plugin — must match expected return types
    case "plugin:store|load":
      return 1; // Resource ID (numeric)
    case "plugin:store|get":
      return [null, false]; // [value, exists] tuple
    case "plugin:store|set":
    case "plugin:store|save":
    case "plugin:store|delete":
    case "plugin:store|clear":
    case "plugin:store|reset":
      return null;
    case "plugin:store|entries":
      return [];
    case "plugin:store|keys":
      return [];
    case "plugin:store|values":
      return [];
    case "plugin:store|length":
      return 0;
    case "plugin:store|has":
      return false;

    // Resource cleanup
    case "plugin:resources|close":
      return null;

    default:
      console.warn(`[mock] Unhandled invoke: ${cmd}`, args);
      return null;
  }
}

// ── Screenplay driver (for video recording pipeline) ────────────────────────

function installScreenplayDriver() {
  // Listen for session updates from Playwright recorder
  window.addEventListener("screenplay:update-session", ((e: CustomEvent) => {
    const { sessionId, updates } = e.detail as {
      sessionId: string;
      updates: Partial<SessionInfo>;
    };
    currentSessions = currentSessions.map((s) =>
      s.id === sessionId ? { ...s, ...updates } : s
    );
    emit("sessions-updated", currentSessions);
  }) as EventListener);

  // Expose API for Playwright to call directly
  (window as any).__screenplay_updateSession = (
    sessionId: string,
    updates: Partial<SessionInfo>,
  ) => {
    window.dispatchEvent(
      new CustomEvent("screenplay:update-session", {
        detail: { sessionId, updates },
      }),
    );
  };

  // Expose API to replace all sessions at once
  (window as any).__screenplay_setSessions = (sessions: SessionInfo[]) => {
    currentSessions = sessions;
    emit("sessions-updated", currentSessions);
  };

  // Expose API to get current sessions (for debugging)
  (window as any).__screenplay_getSessions = () => currentSessions;

  console.log("[mock] Screenplay driver installed");
}

// ── Install ─────────────────────────────────────────────────────────────────

export function installMocks() {
  // Must call mockWindows first to set up __TAURI_INTERNALS__.metadata
  mockWindows("main");

  // Install IPC handler with event mocking enabled
  mockIPC((cmd, args) => handleIPC(cmd, (args ?? {}) as Record<string, unknown>), {
    shouldMockEvents: true,
  });

  // Start ticking sessions every 2s
  setInterval(tickSessions, 2000);

  // Install screenplay driver for video pipeline
  installScreenplayDriver();

  console.log("[mock] Tauri mock layer installed — running in demo mode");
}
