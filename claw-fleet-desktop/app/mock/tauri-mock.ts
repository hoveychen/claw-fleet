/**
 * Mock layer using Tauri's official mocks API.
 * Allows the app to run in a plain browser without the Tauri runtime.
 *
 * Activated by importing and calling installMocks() before any app code.
 */

import { mockIPC, mockWindows } from "@tauri-apps/api/mocks";
import { emit } from "@tauri-apps/api/event";
import type { RawMessage, SessionInfo } from "../types";
import type { LiveThinking } from "../generated/types";
import type { PromoScene } from "./promo-scene";
import { DEMO_BOARD, RELAY_CHAINS, RELAY_PATH_TO_ID, RELAY_SESSIONS } from "./demoData";
import { RELAY_SCRIPTS } from "./demoScripts";
import {
  MOCK_QA_DELAY_MS,
  MOCK_QA_MARKETPLACES,
  MOCK_QA_PLUGINS,
  mockQaDecisionHistory,
  mockQaElicitationRequest,
  shouldDelayMockQaCommand,
} from "./qa";
import {
  MOCK_SESSIONS,
  MOCK_MESSAGES,
  MOCK_CHAT_WORKSPACE,
  mockBrowseDir,
  MOCK_ACCOUNT_INFO,
  MOCK_CODEX_USAGE,
  MOCK_MEMORIES,
  MOCK_WIKI_DOCS,
  MOCK_WIKI_BODIES,
  MOCK_EXPLORER_ROOTS,
  MOCK_EXPLORER_TREE,
  MOCK_HTML_FILE,
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
  MOCK_AUDIT_RULES,
  MOCK_DAILY_REPORT,
  MOCK_HANDOFF_CHAINS,
  MOCK_HEATMAP_STATS,
  MOCK_LESSONS,
  MOCK_MANAGED_LESSONS,
  MOCK_TIMELINE_REPORTS,
  getMessagesForSession,
} from "./data";

// ── Dynamic session state (simulates live updates) ──────────────────────────

// Test harnesses can seed `mock-no-sessions` in localStorage before boot to
// simulate a machine that has never run an agent (onboarding ready-card path).
// `?mock&demo` swaps the standard mock board for the promo screencast board:
// real claude-fleet history (translated) + the 5-hop relay. Gated so existing
// screenshot / footage tooling keeps the original board untouched.
const DEMO_MODE = new URLSearchParams(window.location.search).has("demo");

let currentSessions: SessionInfo[] =
  window.localStorage.getItem("mock-no-sessions") !== null
    ? []
    : DEMO_MODE
      ? [...structuredClone(DEMO_BOARD), ...structuredClone(RELAY_SESSIONS)]
      : structuredClone(MOCK_SESSIONS);

/** Nudge token counts and speeds to simulate live activity */
function tickSessions() {
  currentSessions = currentSessions.map((s) => {
    if (s.status === "idle") return s;
    const jitter = (Math.random() - 0.3) * 5;
    const newSpeed = Math.max(0, s.tokenSpeed + jitter);
    const tokensAdded = Math.round(newSpeed * 2);
    const costPerMin = Math.round(newSpeed * 1.2) / 100;
    return {
      ...s,
      tokenSpeed: Math.round(newSpeed * 10) / 10,
      totalOutputTokens: s.totalOutputTokens + tokensAdded,
      costSpeedUsdPerMin: costPerMin,
      totalCostUsd: Math.round(((s.totalCostUsd ?? 0) + (costPerMin * 2) / 60) * 100) / 100,
      lastActivityMs: Date.now() - Math.random() * 5000,
    };
  });
  // Push update via Tauri event system
  emit("sessions-updated", currentSessions);
}

// ── IPC handler ─────────────────────────────────────────────────────────────

const delay = (ms: number) => new Promise((resolve) => setTimeout(resolve, ms));

/** Command-aware canned LLM risk analysis for the guard card (markdown). */
function guardAnalysisFor(command: string): string {
  if (/rm\s+-rf|drop\s+table|truncate/i.test(command)) {
    return [
      "**What it does:** recursively deletes the target path — in this case a production cache directory that other services read from.",
      "",
      "**Risk: HIGH.** The path is not regenerated automatically; the cache warmer runs nightly. Deleting it now means every request until ~02:00 misses cache and hits the primary DB.",
      "",
      "**Recommendation:** block. If the agent needs a clean cache, `redis-cli FLUSHDB` on the staging instance is the safe equivalent.",
    ].join("\n");
  }
  if (/migrate|prisma|alembic/i.test(command)) {
    return [
      "**What it does:** applies all pending migrations to the production database, altering live schema.",
      "",
      "**Risk: MEDIUM.** 2 of 3 pending migrations are additive; one drops `legacy_plan`. Writers still referencing that column would fail mid-deploy.",
      "",
      "**Recommendation:** allow once **after** confirming the 14:00 snapshot completed — the agent's plan already gates the drop behind a feature flag.",
    ].join("\n");
  }
  return [
    "**What it does:** " + command.split("\n")[0] + "",
    "",
    "**Risk: LOW.** No destructive flags detected; the command only touches the workspace sandbox.",
    "",
    "**Recommendation:** safe to allow once.",
  ].join("\n");
}

let spawnCounter = 0;

// Remote-workspace registry (rca), in-memory so the settings section's
// add/remove flow is exercisable in ?mock screenshots.
let mockRemoteWorkspaces: {
  path: string;
  pairingCode?: string;
  sshTarget?: string;
  remoteRcaPath?: string;
  label?: string;
}[] = [
  { path: "/Users/dev/remote-api", pairingCode: "rca1.JgAkCAESIPuTmockmockmock", label: "gpu-box" },
];

function handleIPC(
  cmd: string,
  args: Record<string, unknown> = {},
  qaMode = false,
): unknown {
  switch (cmd) {
    case "list_sessions":
      return currentSessions;
    case "get_messages": {
      const jsonlPath = args.jsonlPath as string;
      const relayId = RELAY_PATH_TO_ID[jsonlPath];
      // Relay hero session: hand back only the opening prompt; the rest streams
      // in via `session-tail` once `start_watching_session` starts the driver.
      if (relayId && RELAY_SCRIPTS[relayId]) return RELAY_SCRIPTS[relayId].slice(0, 1);
      const session = currentSessions.find((s) => s.jsonlPath === jsonlPath);
      return getMessagesForSession(session?.id ?? "");
    }
    case "get_messages_tail": {
      const jsonlPath = args.jsonlPath as string;
      const tail = (args.tail as number) ?? 500;
      const relayId = RELAY_PATH_TO_ID[jsonlPath];
      if (relayId && RELAY_SCRIPTS[relayId]) return RELAY_SCRIPTS[relayId].slice(0, 1);
      const session = currentSessions.find((s) => s.jsonlPath === jsonlPath);
      const all = getMessagesForSession(session?.id ?? "");
      return all.slice(Math.max(0, all.length - tail));
    }
    // Relay streaming engine hooks (promo-mock-demo).
    case "read_live_thinking": {
      const sid = args.sessionId as string;
      return relayStream && relayStream.sessionId === sid ? relayStream.liveThinking : null;
    }
    case "start_watching_session": {
      const jsonlPath = args.jsonlPath as string;
      const relayId = RELAY_PATH_TO_ID[jsonlPath];
      if (relayId) startRelayStream(relayId);
      return null;
    }
    case "stop_watching_session":
      stopRelayStream(true);
      return null;
    case "get_tool_result_full": {
      // Pairs with the trimmed image Read in mock data (tool-img-trimmed):
      // return the untrimmed payload the way the Rust extractor would.
      if (args.toolUseId === "tool-img-trimmed") {
        return {
          content: [
            {
              type: "image",
              source: {
                type: "base64",
                media_type: "image/png",
                data: "iVBORw0KGgoAAAANSUhEUgAAADAAAAAwCAIAAADYYG7QAAAAQklEQVR4nO3OQQ0AIAwAsUlEIhKRgAuOR5MK6Kx9vjL5QEhISKgeCAkJCdUDISEhoXogJCQkVA+EhISE6oGQkNBjF+yE4PHth+9DAAAAAElFTkSuQmCC",
              },
            },
          ],
          toolUseResult: null,
        };
      }
      throw new Error(`tool_use_id ${args.toolUseId as string} not found`);
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
    case "set_keep_awake":
      return null;

    // Keep-awake (coffee) toggle — pretend the platform supports it so the
    // sidebar footer button renders under ?mock.
    case "keep_awake_supported":
      return true;
    case "get_keep_awake":
      return false;

    case "set_session_title": {
      // Mirror the desktop restamp: set/clear titleOverride, re-emit so the row
      // re-renders with the new (or reverted) title.
      const sessionId = args.sessionId as string;
      const rawTitle = args.title as string | null | undefined;
      const title = rawTitle?.trim() || null;
      currentSessions = currentSessions.map((s) =>
        s.id === sessionId ? { ...s, titleOverride: title } : s,
      );
      emit("sessions-updated", currentSessions);
      return null;
    }

    case "list_skills":
      return MOCK_SKILLS;
    case "skill_sync_inventory":
      return MOCK_SKILLS.map((skill) => ({
        slug: skill.name,
        state: "unmanaged",
        compatibility: "both",
        warnings: [],
        canonicalPath: null,
        claudePath: skill.path,
        codexPath: null,
        claudeManaged: false,
        codexManaged: false,
      }));
    case "skill_sync_apply":
    case "skill_sync_adopt":
      return { items: [], actions: [], conflicts: [] };
    case "skill_sync_unlink":
      return { slug: args.slug, target: args.target, action: "unlinked", path: "" };
    case "get_skill_autosync":
      return false;
    case "set_skill_autosync":
      return null;
    // The pure-chat workspace. Returning it (rather than falling through to
    // `default: return null`) is what makes the launchpad's chat filter and the
    // new-session form's pinned chat entry appear in mock mode.
    case "chat_workspace":
      return MOCK_CHAT_WORKSPACE;

    // Launcher's remote directory picker. Under `?mock&remote` this stands in
    // for the probe host's filesystem.
    case "browse_dir":
      return mockBrowseDir((args as { path?: string | null })?.path);

    case "list_skill_files":
      return MOCK_SKILL_FILES;
    case "get_skill_content":
      return "# muveectl\n\nmock 模式下的占位正文。";

    // File explorer (Repos view + the scratchpad tab).
    case "list_explorer_roots":
      return MOCK_EXPLORER_ROOTS;
    case "list_explorer_dir":
      return MOCK_EXPLORER_TREE[(args.relativePath as string) ?? ""] ?? [];
    case "read_explorer_file": {
      const rel = (args.relPath as string) ?? "";
      // Real HTML (with inline styles + a script) so the iframe preview branch
      // has something to actually render in ?mock.
      if (rel.endsWith(".html") || rel.endsWith(".htm")) {
        return { kind: "text", content: MOCK_HTML_FILE, truncated: false, sizeBytes: MOCK_HTML_FILE.length };
      }
      return { kind: "text", content: "mock 模式下的占位文件内容。\n", truncated: false, sizeBytes: 42 };
    }
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
      return MOCK_WIKI_BODIES[(args.slug as string) ?? ""]
        ?? "# Not published\n\nThis document has no mock body.";
    case "read_review_doc": {
      const doc = (args.doc ?? {}) as { kind?: string; ref?: string; title?: string };
      if (doc.kind === "wiki") {
        return {
          format: "markdown",
          title: doc.title || (doc.ref ?? "doc"),
          body: `# Rollback runbook\n\nIf the cutover misbehaves, flip the \`dual_write\` flag back **on** — no restore needed.\n\n1. \`fleetctl flag set dual_write=on\`\n2. Confirm lag < 5s on the replica\n3. Page #billing-oncall\n\n> Rollback is a flag flip, not a data migration.`,
        };
      }
      return {
        format: "markdown",
        title: doc.title || "cutover-plan.md",
        body: `# Cutover plan\n\n## Backfill\n- 2.1M rows validated, **0 drift**\n- dual-write bake: 48h\n\n## Steps\n1. Freeze writes on \`usage_events\` (~40s lock)\n2. Swap read path to the new table\n3. Keep dual-write on for 48h\n\n\`\`\`sql\nUPDATE plans SET migrated = true WHERE id > 0;\n\`\`\`\n\nSee [[billing/rollback-runbook]] for the abort path.`,
      };
    }
    case "list_plugins":
      return qaMode ? MOCK_QA_PLUGINS : [];
    case "list_marketplaces":
      return qaMode ? MOCK_QA_MARKETPLACES : [];
    case "list_workspace_procs":
      return [];
    case "clear_workspace_procs":
      return 0;
    case "search_sessions": {
      // Real-shaped FTS over the mock transcripts: match any message whose
      // JSON carries the query, so the search → open-with-query → fold
      // auto-expand path is exercisable in mock mode.
      const q = String(args.query ?? "").toLowerCase();
      if (q.length < 2) return [];
      const hits: unknown[] = [];
      for (const [sid, msgs] of Object.entries(MOCK_MESSAGES)) {
        const hit = msgs.some((m) => JSON.stringify(m.message?.content ?? "").toLowerCase().includes(q));
        if (!hit) continue;
        const sess = MOCK_SESSIONS.find((s) => s.id === sid);
        if (!sess) continue;
        hits.push({ sessionId: sid, jsonlPath: sess.jsonlPath, snippet: q, rank: hits.length });
      }
      return hits;
    }
    case "resume_fleet_session":
    case "resume_rate_limited_session":
      return null;
    case "get_workflow_trees":
      return [];
    case "list_session_decisions":
      return qaMode
        ? mockQaDecisionHistory(String(args.sessionId ?? ""))
        : [];
    case "get_task_plans":
      return [];
    case "get_platform":
      return "macos";
    case "get_waiting_alerts":
      return MOCK_WAITING_ALERTS;
    case "get_account_info":
      return MOCK_ACCOUNT_INFO;
    case "get_source_account": {
      return null;
    }
    case "get_source_usage": {
      const source = args.source as string;
      if (source === "codex") return MOCK_CODEX_USAGE;
      return null;
    }
    case "list_loops":
      return [
        {
          id: "a1b2c3d4",
          workspacePath: "/Users/dev/workspace/netferry",
          prompt: "检查 CI 是否有夜间构建失败，有的话汇总失败的 job 和错误摘要。",
          intervalSecs: 86400,
          nextFireAt: Date.now() + 5 * 3600 * 1000,
          iterationsDone: 3,
          model: "claude-opus-4-8",
          agentSource: "claude",
        },
      ];
    case "list_schedules":
      return [
        {
          id: "9f8e7d6c",
          workspacePath: "/Users/dev/workspace/claude-fleet",
          prompt: "5 天后回来检查 fleet schedule 上线后的真实使用情况，看有没有 stranded 的计时器需要处理。",
          fireAt: Date.now() + 3 * 86400 * 1000 + 4 * 3600 * 1000,
          status: "pending",
          model: "claude-opus-4-8",
          agentSource: "claude",
        },
        {
          id: "1a2b3c4d",
          workspacePath: "/Users/dev/workspace/talk-cli",
          prompt: "发布后巡检：确认 npm 包已可安装并跑通 smoke。",
          fireAt: Date.now() - 2 * 3600 * 1000,
          status: "fired",
          firedAt: Date.now() - 2 * 3600 * 1000,
          firedSessionId: "sess-abc123",
          agentSource: "codex",
        },
      ];
    case "cancel_loop":
    case "cancel_schedule":
      return null;
    case "update_schedule": {
      // Echo back a plausibly-updated record so the edit form's round-trip works
      // in mock mode (real backend re-arms the timer; the mock just reflects).
      const u = (args?.update ?? {}) as Record<string, unknown>;
      return {
        id: u.id ?? "9f8e7d6c",
        workspacePath: "/Users/dev/workspace/claude-fleet",
        prompt: u.prompt ?? "",
        fireAt: u.fireAt ?? Date.now() + 3600 * 1000,
        status: "pending",
        model: u.model || undefined,
        effort: u.effort || undefined,
        agentSource: u.agentSource || undefined,
      };
    }
    case "list_memories":
      return qaMode
        ? MOCK_MEMORIES.map((workspace) => ({
            ...workspace,
            source: "claude-code",
          }))
        : MOCK_MEMORIES;
    case "get_memory_content":
      return MOCK_MEMORY_CONTENT;
    case "get_memory_history":
      return MOCK_MEMORY_HISTORY;
    case "get_sources_config":
      return MOCK_SOURCES_CONFIG;

    // Codex profile-v2 files on the host. Two entries so the model picker
    // shows the third-party half in screenshots; the real list comes from
    // `<CODEX_HOME>/*.config.toml`.
    case "list_codex_profiles":
      return [
        {
          name: "deepseek-flash",
          model: "deepseek/deepseek-v4-flash",
          model_provider: "openrouter",
          reasoning_effort: "low",
        },
        {
          name: "deepseek-pro",
          model: "deepseek/deepseek-v4-pro",
          model_provider: "openrouter",
          reasoning_effort: null,
        },
      ];

    // The settings panel dereferences these on render (`llmProviders.find`),
    // so the default `return null` crashed the whole settings page in mock
    // mode. Minimal truthy shapes keep it renderable for screenshots.
    case "get_auto_resume_config":
      return { enabled: true, maxWaitHours: 12 };

    case "list_llm_providers":
      return [
        {
          name: "claude",
          displayName: "Claude Code",
          available: true,
          models: [],
          defaultFastModel: "haiku",
          defaultStandardModel: "sonnet",
        },
      ];
    case "get_llm_config":
      return {
        provider: "claude",
        fastModel: "haiku",
        standardModel: "sonnet",
        dailyReportPreference: "claude",
      };

    case "list_remote_workspaces":
      return { workspaces: mockRemoteWorkspaces };
    case "upsert_remote_workspace": {
      const entry = args.entry as { path: string; pairingCode: string; label?: string };
      if (!entry.pairingCode.startsWith("rca1.") || entry.pairingCode.length <= 5) {
        throw new Error(
          "pairing code must be the 'rca1.…' string printed by `rca serve` on the remote host",
        );
      }
      mockRemoteWorkspaces = [
        ...mockRemoteWorkspaces.filter((w) => w.path !== entry.path),
        entry,
      ];
      return { workspaces: mockRemoteWorkspaces };
    }
    case "remove_remote_workspace": {
      mockRemoteWorkspaces = mockRemoteWorkspaces.filter(
        (w) => w.path !== (args.path as string),
      );
      return { workspaces: mockRemoteWorkspaces };
    }
    case "update_rca_remote": {
      // No-op in mock: the entry already exists; just return the registry.
      return { workspaces: mockRemoteWorkspaces };
    }
    case "install_rca_remote": {
      const conn = args.conn as { sshProfile?: string | null; username?: string; host?: string };
      const path = args.path as string;
      const label = (args.label as string | null) ?? undefined;
      const sshTarget = conn.sshProfile || `${conn.username ?? "user"}@${conn.host ?? "host"}`;
      mockRemoteWorkspaces = [
        ...mockRemoteWorkspaces.filter((w) => w.path !== path),
        { path, sshTarget, remoteRcaPath: "/home/user/.fleet/bin/rca", label },
      ];
      return { workspaces: mockRemoteWorkspaces };
    }
    case "check_setup_status": {
      // Harness override: seed `mock-setup-status` with a JSON object to merge
      // over the default (e.g. {"cli_installed":false}) for onboarding-branch
      // screenshots. `mock-setup-delay` (ms) delays the response to exercise
      // the diagnostics-timeout fallback.
      const override = window.localStorage.getItem("mock-setup-status");
      const result = override
        ? { ...MOCK_SETUP_STATUS, ...JSON.parse(override) }
        : MOCK_SETUP_STATUS;
      const delayMs = Number(window.localStorage.getItem("mock-setup-delay") ?? 0);
      return delayMs > 0
        ? new Promise((resolve) => setTimeout(() => resolve(result), delayMs))
        : result;
    }
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
    case "get_audit_rules":
      return MOCK_AUDIT_RULES;
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
    case "list_managed_lessons":
      return MOCK_MANAGED_LESSONS;
    case "remove_managed_lesson":
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

    // ── Today's cumulative spend (sidebar badge) ──
    case "today_usage":
      return {
        date: new Date().toISOString().slice(0, 10),
        outputTokens: 1_284_500,
        costUsd: 23.87,
        agentCostUsd: 23.87,
        fleetCostUsd: 0,
        sessionCount: 37,
      };

    // ── Per-model receipt behind the sidebar badge ──
    case "today_usage_breakdown":
      return {
        date: new Date().toISOString().slice(0, 10),
        lines: [
          {
            model: "claude-opus-4-8",
            source: "claude-code",
            inputTokens: 184_200,
            // Claude Code opts into the 1-hour cache TTL, so a realistic line
            // carries its writes in the 1h bucket. costUsd is the exact sum of
            // the rows below — the receipt's reconciliation invariant.
            cacheCreationTokens: 0,
            cacheCreation1hTokens: 512_000,
            cacheReadTokens: 8_940_000,
            outputTokens: 342_500,
            inputPrice: 5.0,
            outputPrice: 25.0,
            cacheWritePrice: 6.25,
            cacheWrite1hPrice: 10.0,
            cacheReadPrice: 0.5,
            costUsd: 19.0735,
          },
          {
            model: "gpt-5.6-sol",
            source: "codex",
            inputTokens: 96_400,
            cacheCreationTokens: 0,
            cacheCreation1hTokens: 0,
            cacheReadTokens: 1_240_000,
            outputTokens: 78_300,
            inputPrice: 5.0,
            outputPrice: 30.0,
            cacheWritePrice: 6.25,
            cacheWrite1hPrice: 6.25,
            cacheReadPrice: 0.5,
            costUsd: 3.451,
          },
        ],
        totalInputTokens: 280_600,
        totalCacheCreationTokens: 512_000,
        totalCacheReadTokens: 10_180_000,
        totalOutputTokens: 420_800,
        totalCostUsd: 22.5245,
        agentCostUsd: 22.5245,
        fleetCostUsd: 0,
      };

    // ── Guard LLM analysis (feeds the "Analyzing command…" beat) ──
    case "get_guard_context":
      return "The agent just finished the migration plan review and asked to apply it. Last assistant message: \"All 3 migrations reviewed; the drop is gated behind usage_billing_v2. Requesting approval to deploy.\"";
    case "analyze_guard_command":
      return delay(1400).then(() => guardAnalysisFor((args.command as string) ?? ""));

    // ── Handoff chains (接力 chip + expanded panel) ──
    case "get_handoff_chain": {
      const sid = args.sessionId as string;
      const sess = currentSessions.find((s) => s.id === sid);
      const chainId = sess?.handoff?.chainId;
      return (chainId && (RELAY_CHAINS[chainId] || MOCK_HANDOFF_CHAINS[chainId])) || null;
    }

    // ── Mobile relay (Mobile 板块; static demo values) ──
    case "get_mobile_relay_config":
    case "set_mobile_relay_config":
    case "rotate_mobile_relay_secret":
      return {
        enabled: true,
        relayUrl: "https://fleet-relay.example.com",
        secret: "demo-pairing-secret",
      };
    case "mobile_relay_status":
      return {
        enabled: true,
        connected: true,
        clients: 2,
        relayUrl: "https://fleet-relay.example.com",
        secretSet: true,
        devices: [
          { clientId: "dev-iphone", label: "iPhone 15 Pro", platform: "ios", pushSubscribed: true, connectedAtMs: Date.now() - 3_600_000, lastSeenMs: Date.now() - 4_000, appCommit: "abc1234" },
          { clientId: "dev-android", label: "Pixel 9", platform: "android", pushSubscribed: true, connectedAtMs: Date.now() - 7_200_000, lastSeenMs: Date.now() - 65_000, appCommit: "0000000" },
        ],
      };
    // Desktop build commit; the iPhone above matches (fresh) and the Pixel does
    // not (stale) so the mock exercises both banner states.
    case "desktop_build_commit":
      return "abc1234";
    case "mobile_relay_qr_svg":
      return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 29 29" shape-rendering="crispEdges"><rect width="29" height="29" fill="#fff"/><path fill="#000" d="M2 2h7v7H2zM20 2h7v7h-7zM2 20h7v7H2zM4 4h3v3H4zM22 4h3v3h-4zM4 22h3v3H4zM11 2h2v2h-2zM15 2h2v3h-2zM11 6h3v2h-3zM16 6h2v2h-2zM11 10h2v3h-2zM15 11h3v2h-3zM20 11h3v2h-3zM25 11h2v3h-2zM2 11h3v2H2zM6 12h3v2H6zM11 15h2v3h-2zM14 16h3v2h-3zM19 15h2v3h-2zM23 16h2v2h-2zM26 16h1v3h-1zM11 20h3v2h-3zM16 21h2v3h-2zM20 20h3v2h-3zM24 21h3v2h-3zM11 24h2v3h-2zM14 25h3v2h-3zM20 24h2v3h-2zM23 25h3v2h-3z"/></svg>`;

    // ── Dispatch (New Session form) — spawn a fake session onto the board ──
    case "spawn_new_claude_session": {
      spawnCounter += 1;
      const ws = (args.workspacePath as string) ?? "/Users/demo/workspace/new-task";
      const name = ws.split("/").filter(Boolean).pop() ?? "new-task";
      const id = `sess-spawned-${spawnCounter}`;
      const prompt = (args.prompt as string) ?? "";
      const spawned: SessionInfo = {
        ...structuredClone(MOCK_SESSIONS[0]),
        id,
        workspacePath: ws,
        workspaceName: name,
        ideName: null,
        isSubagent: false,
        parentSessionId: null,
        agentType: null,
        agentDescription: null,
        slug: null,
        aiTitle: prompt.split("\n")[0].slice(0, 72) || "New dispatched task",
        status: "thinking",
        tokenSpeed: 6 + Math.random() * 8,
        agentTokenSpeed: 0,
        totalOutputTokens: 0,
        lastMessagePreview: "Reading the workspace and drafting a plan...",
        lastActivityMs: Date.now(),
        agentLastActivityMs: Date.now(),
        runningSubagentCount: 0,
        createdAtMs: Date.now(),
        jsonlPath: `/Users/demo/.claude/projects/${name}/${id}.jsonl`,
        contextPercent: 0.01,
        handoff: null,
      };
      currentSessions = [spawned, ...currentSessions];
      emit("sessions-updated", currentSessions);
      return { pid: 90000 + spawnCounter, sessionId: id };
    }

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

// ── Relay streaming engine (promo-mock-demo) ────────────────────────────────
// Replays a hero relay session's screenplay when its detail view is opened:
// streams each assistant turn's `thinking` through `read_live_thinking`, then
// appends the completed message via a `session-tail` event — exactly the wire
// the real backend watcher drives. Only one stream runs at a time (the global
// detail store closes the previous session before opening the next).

interface RelayStream {
  sessionId: string;
  script: RawMessage[];
  playhead: number; // next screenplay index to reveal
  liveThinking: LiveThinking | null;
  originalStatus: SessionInfo["status"];
  timer: number | null;
  pendingThinking: string | null; // thinking text mid-stream, null when idle
  thinkingShown: number; // chars of pendingThinking revealed so far
}
let relayStream: RelayStream | null = null;

// Fast-forward pacing: a beat every ~380ms, thinking revealed ~90 chars/beat,
// so a ~10-message hop plays in ~15s — a believable sped-up session.
const RELAY_BEAT_MS = 380;
const RELAY_THINK_CHARS = 90;

function relayFirstThinking(msg: RawMessage): string | null {
  const c = msg.message?.content;
  if (Array.isArray(c)) {
    for (const b of c) {
      if (b && typeof b === "object" && (b as { type?: string }).type === "thinking") {
        return (b as { thinking?: string }).thinking ?? null;
      }
    }
  }
  return null;
}

function stopRelayStream(restoreStatus: boolean) {
  if (!relayStream) return;
  const s = relayStream;
  if (s.timer != null) clearInterval(s.timer);
  if (restoreStatus) {
    currentSessions = currentSessions.map((x) =>
      x.id === s.sessionId ? { ...x, status: s.originalStatus } : x,
    );
    emit("sessions-updated", currentSessions);
  }
  relayStream = null;
}

function startRelayStream(sessionId: string) {
  stopRelayStream(true);
  const script = RELAY_SCRIPTS[sessionId];
  if (!script || script.length === 0) return;
  const sess = currentSessions.find((x) => x.id === sessionId);
  const originalStatus = sess?.status ?? "idle";
  // Flip to a live status so the board card animates and reads as active while
  // the detail streams. The live-thinking poller already arms (procAlive), so
  // this is purely visual; the original status is restored when the stream ends.
  currentSessions = currentSessions.map((x) =>
    x.id === sessionId ? { ...x, status: "streaming", tokenSpeed: 74 } : x,
  );
  emit("sessions-updated", currentSessions);
  relayStream = {
    sessionId,
    script,
    playhead: 1, // index 0 (the opening prompt) is returned by get_messages_tail
    liveThinking: null,
    originalStatus,
    timer: window.setInterval(tickRelayStream, RELAY_BEAT_MS),
    pendingThinking: null,
    thinkingShown: 0,
  };
}

function tickRelayStream() {
  const s = relayStream;
  if (!s) return;

  // Streaming a thinking block token-by-token.
  if (s.pendingThinking != null) {
    s.thinkingShown = Math.min(s.pendingThinking.length, s.thinkingShown + RELAY_THINK_CHARS);
    s.liveThinking = {
      sessionId: s.sessionId,
      thinking: s.pendingThinking.slice(0, s.thinkingShown),
      streaming: true,
      updatedSecsAgo: 0,
    };
    if (s.thinkingShown >= s.pendingThinking.length) {
      // Thinking complete → land the full message, drop the live bubble.
      emit("session-tail", [s.script[s.playhead]]);
      s.playhead += 1;
      s.pendingThinking = null;
      s.thinkingShown = 0;
      s.liveThinking = null;
    }
    return;
  }

  // End of screenplay → restore status, stop.
  if (s.playhead >= s.script.length) {
    stopRelayStream(true);
    return;
  }

  const msg = s.script[s.playhead];
  const thinking = msg.type === "assistant" ? relayFirstThinking(msg) : null;
  if (thinking) {
    s.pendingThinking = thinking;
    s.thinkingShown = Math.min(thinking.length, RELAY_THINK_CHARS);
    s.liveThinking = {
      sessionId: s.sessionId,
      thinking: thinking.slice(0, s.thinkingShown),
      streaming: true,
      updatedSecsAgo: 0,
    };
  } else {
    // Tool-use turn, tool_result, or plain text: land it immediately.
    emit("session-tail", [msg]);
    s.playhead += 1;
    s.liveThinking = null;
  }
}

// ── Install ─────────────────────────────────────────────────────────────────

export function installMocks({ qaMode = false }: { qaMode?: boolean } = {}) {
  // Must call mockWindows first to set up __TAURI_INTERNALS__.metadata
  mockWindows("main");

  // Install IPC handler with event mocking enabled
  mockIPC(async (cmd, args) => {
    if (qaMode && shouldDelayMockQaCommand(cmd)) {
      await delay(MOCK_QA_DELAY_MS);
    }
    return handleIPC(cmd, (args ?? {}) as Record<string, unknown>, qaMode);
  }, {
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
  // v2 fleet__ask card: rich HTML preview + dynamic form fields + options.
  (window as any).__mock_fleet_ask = (overrides: Record<string, unknown> = {}) => {
    const id = `mock-ask-${Date.now()}`;
    emit("fleet-ask-request", {
      id,
      sessionId: "sess-billing-3",
      workspaceName: "billing-service",
      aiTitle: "Cutover plan ready — pick the rollout window",
      timestamp: new Date().toISOString(),
      reviewDocs: [
        { kind: "file", ref: "/Users/demo/billing-service/docs/cutover-plan.md", title: "cutover-plan.md" },
        { kind: "wiki", ref: "billing/rollback-runbook", title: "Rollback runbook" },
      ],
      questions: [
        {
          question: "Backfill is validated (2.1M rows, 0 drift). Review the cutover impact below, leave a note for the status page, and pick the window.",
          header: "Cutover",
          multiSelect: false,
          html: `<div style="font-family:-apple-system,Segoe UI,sans-serif;font-size:13px;color:#1f2023">
  <table style="border-collapse:collapse;width:100%">
    <tr style="text-align:left;border-bottom:2px solid #e7e4dd"><th style="padding:6px 8px">table</th><th style="padding:6px 8px">rows</th><th style="padding:6px 8px">est. lock</th></tr>
    <tr style="border-bottom:1px solid #e7e4dd"><td style="padding:6px 8px;font-family:monospace">usage_events</td><td style="padding:6px 8px">2.1M</td><td style="padding:6px 8px">~40s</td></tr>
    <tr style="border-bottom:1px solid #e7e4dd"><td style="padding:6px 8px;font-family:monospace">invoices</td><td style="padding:6px 8px">380K</td><td style="padding:6px 8px">~6s</td></tr>
    <tr><td style="padding:6px 8px;font-family:monospace">plans</td><td style="padding:6px 8px">1.2K</td><td style="padding:6px 8px">&lt;1s</td></tr>
  </table>
  <p style="margin:10px 0 0;color:#b45309">⚠ dual-write stays on for 48h — rollback is a flag flip, not a restore.</p>
</div>`,
          formFields: [
            { name: "status_note", kind: "textarea", label: "Status-page note", placeholder: "What customers will see during the window", required: false },
            { name: "bake_hours", kind: "range", label: "Dual-write bake time (hours)", min: 12, max: 72, step: 12, default: 48 },
          ],
          options: [
            { label: "Tonight 02:00 UTC", description: "Lowest traffic; on-call is already scheduled." },
            { label: "Saturday 06:00 UTC", description: "More slack, but pushes the release train by 2 days." },
          ],
        },
      ],
      ...overrides,
    });
    return id;
  };
  // Native Claude Code permission prompt routed via fleet__permission_prompt.
  // Pass overrides to exercise each tool shape, e.g.
  //   __mock_permission_prompt({ toolName: "Read", toolInput: { file_path: "/a/b/c.ts" } })
  (window as any).__mock_permission_prompt = (overrides: Record<string, unknown> = {}) => {
    const id = `mock-perm-${Date.now()}`;
    emit("permission-prompt-request", {
      id,
      sessionId: "sess-fleet-main",
      workspaceName: "claw-fleet",
      aiTitle: "Needs approval to touch the filesystem",
      timestamp: new Date().toISOString(),
      toolName: "Edit",
      toolInput: {
        file_path: "/Users/demo/workspace/claw-fleet/claw-fleet-desktop/src/gui/mod.rs",
        old_string: "let x = 1;",
        new_string: "let x = 2;",
      },
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

  (window as any).__mock_qa_elicitation = () => {
    const request = mockQaElicitationRequest();
    emit("elicitation-request", request);
    return request.id;
  };

  console.log("[mock] Tauri mock layer installed — running in demo mode");
  console.log("[mock] Trigger decisions via __mock_guard() / __mock_elicitation() in DevTools");
}

export function triggerMockQaScenario(): void {
  const driver = (window as unknown as { __mock_qa_elicitation?: () => string })
    .__mock_qa_elicitation;
  if (!driver) throw new Error("mock QA driver was not installed");
  driver();
}

/** Fire a promo scene through the event bus installed during this exact boot. */
export function triggerPromoScene(scene: PromoScene): void {
  if (scene === "base") return;
  if (scene === "guard") {
    const id = `promo-guard-${Date.now()}`;
    emit("guard-request", {
      id,
      sessionId: "sess-api-main",
      workspaceName: "api-server",
      aiTitle: "Wants to deploy the database migration",
      toolName: "Bash",
      command: "npx prisma migrate deploy",
      commandSummary: "Apply pending migrations to production",
      riskTags: ["database", "production"],
      timestamp: new Date().toISOString(),
    });
    return;
  }

  const driver = (window as unknown as { __mock_fleet_ask?: () => string }).__mock_fleet_ask;
  if (!driver) throw new Error("promo ask driver was not installed");
  driver();
}
