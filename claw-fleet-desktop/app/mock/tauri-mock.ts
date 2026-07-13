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
  MOCK_CHAT_WORKSPACE,
  MOCK_ACCOUNT_INFO,
  MOCK_CURSOR_USAGE,
  MOCK_CODEX_USAGE,
  MOCK_OPENCLAW_USAGE,
  MOCK_OPENCLAW_ACCOUNT,
  MOCK_MEMORIES,
  MOCK_WIKI_DOCS,
  MOCK_EXPLORER_ROOTS,
  MOCK_EXPLORER_TREE,
  MOCK_GIT_STATUS,
  MOCK_SKILLS,
  MOCK_SKILL_FILES,
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
    case "get_messages_tail": {
      const jsonlPath = args.jsonlPath as string;
      const tail = (args.tail as number) ?? 500;
      const session = currentSessions.find((s) => s.jsonlPath === jsonlPath);
      const all = getMessagesForSession(session?.id ?? "");
      return all.slice(Math.max(0, all.length - tail));
    }
    case "start_watching_session":
    case "stop_watching_session":
    case "set_locale":
    case "disconnect_remote":
    case "set_source_enabled":
    case "apply_hooks_setup":
    case "interrupt_session":
    case "kill_session":
    case "kill_workspace_sessions":
    case "delete_connection":
    case "connect_remote":
    case "set_lite_mode":
    case "show_main_window":
    case "respond_to_guard":
    case "respond_to_elicitation":
      return null;

    case "list_skills":
      return MOCK_SKILLS;
    // The pure-chat workspace. Returning it (rather than falling through to
    // `default: return null`) is what makes the launchpad's chat filter and the
    // new-session form's pinned chat entry appear in mock mode.
    case "chat_workspace":
      return MOCK_CHAT_WORKSPACE;

    case "list_skill_files":
      return MOCK_SKILL_FILES;
    case "get_skill_content":
      return "# muveectl\n\nmock 模式下的占位正文。";

    // File explorer (Repos view + the scratchpad tab).
    case "list_explorer_roots":
      return MOCK_EXPLORER_ROOTS;
    case "list_explorer_dir":
      return MOCK_EXPLORER_TREE[(args.relativePath as string) ?? ""] ?? [];
    case "read_explorer_file":
      return { kind: "text", content: "mock 模式下的占位文件内容。\n", truncated: false, sizeBytes: 42 };
    case "git_status":
      return MOCK_GIT_STATUS;

    // Wiki / Plugins. These MUST return a list rather than falling through to
    // `default: return null` — both views do `setState(await invoke(...))` and
    // then filter the result, so a null blanks the whole app.
    case "list_wiki_docs":
      return MOCK_WIKI_DOCS;
    case "search_wiki_docs":
      return [];
    case "get_wiki_file_text":
      return "# 架构总览\n\nmock 模式下的占位正文。";
    case "list_plugins":
      return [];
    case "list_marketplaces":
      return [];
    case "list_workspace_procs":
      return [];
    case "clear_workspace_procs":
      return 0;
    case "search_sessions":
      return [];
    case "resume_fleet_session":
    case "resume_rate_limited_session":
      return null;
    case "get_workflow_trees":
      return [];
    case "list_session_decisions":
      return [];
    case "get_task_plans":
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

    // Store plugin — must match expected return types. Backed by
    // localStorage so demo-mode preferences survive reloads and test
    // harnesses can pre-seed keys (e.g. skip onboarding) before boot.
    case "plugin:store|load":
      return 1; // Resource ID (numeric)
    case "plugin:store|get": {
      const v = window.localStorage.getItem(`mock-store:${args.key as string}`);
      return [v, v !== null]; // [value, exists] tuple
    }
    case "plugin:store|set":
      window.localStorage.setItem(`mock-store:${args.key as string}`, String(args.value));
      return null;
    case "plugin:store|delete":
      window.localStorage.removeItem(`mock-store:${args.key as string}`);
      return null;
    case "plugin:store|save":
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

  // Decision-panel drivers — let developers trigger guard / elicitation
  // decisions from the DevTools console to exercise the full-screen takeover
  // (especially useful for the lite portrait mode).
  (window as any).__mock_guard = (overrides: Record<string, unknown> = {}) => {
    const id = `mock-guard-${Date.now()}`;
    emit("guard-request", {
      id,
      sessionId: "sess-fleet-main",
      workspaceName: "claw-fleet",
      aiTitle: "Trying to rm -rf the universe",
      toolName: "Bash",
      command: "rm -rf /",
      commandSummary: "Delete root filesystem",
      riskTags: ["destructive", "filesystem"],
      timestamp: new Date().toISOString(),
      ...overrides,
    });
    return id;
  };
  (window as any).__mock_elicitation = (overrides: Record<string, unknown> = {}) => {
    const id = `mock-elic-${Date.now()}`;
    emit("elicitation-request", {
      id,
      sessionId: "sess-fleet-main",
      workspaceName: "claw-fleet",
      aiTitle: "Which approach should I take?",
      timestamp: new Date().toISOString(),
      questions: [
        {
          question: "要走哪条路？",
          header: "路线",
          multiSelect: false,
          options: [
            { label: "快而脏", description: "耦合紧，速度快" },
            { label: "慢而干净", description: "保持边界，重构成本高" },
          ],
        },
      ],
      ...overrides,
    });
    return id;
  };

  console.log("[mock] Tauri mock layer installed — running in demo mode");
  console.log("[mock] Trigger decisions via __mock_guard() / __mock_elicitation() in DevTools");
}
