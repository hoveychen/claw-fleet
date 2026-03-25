/**
 * Mock data for demo / screenshot mode.
 * Provides realistic session data covering multiple agent types,
 * statuses, and workspaces to showcase all core features.
 */

import type { RawMessage, SessionInfo, SkillInvocation, WaitingAlert } from "../types";

const NOW = Date.now();
const MIN = 60_000;
const HOUR = 3_600_000;

// ── Sessions ────────────────────────────────────────────────────────────────

export const MOCK_SESSIONS: SessionInfo[] = [
  // ── 1. Active main session: "claude-fleet" (this project) — thinking ──
  {
    id: "sess-fleet-main",
    workspacePath: "/Users/demo/workspace/claude-fleet",
    workspaceName: "claude-fleet",
    ideName: "VS Code",
    isSubagent: false,
    parentSessionId: null,
    agentType: null,
    agentDescription: null,
    slug: "feat/mock-mode",
    aiTitle: "Implement mock mode for demo screenshots",
    status: "delegating",
    tokenSpeed: 12.3,
    totalOutputTokens: 48720,
    lastMessagePreview: "Creating mock data module with realistic sessions...",
    lastActivityMs: NOW - 5000,
    createdAtMs: NOW - 25 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/claude-fleet/sess-fleet-main.jsonl",
    model: "claude-opus-4-20250805",
    thinkingLevel: null,
    pid: 12345,
    pidPrecise: true,
    lastSkill: null,
    agentSource: "claude-code",
    lastOutcome: ["feature_added"],
  },
  // Subagent: Explore agent for claude-fleet
  {
    id: "sess-fleet-explore",
    workspacePath: "/Users/demo/workspace/claude-fleet",
    workspaceName: "claude-fleet",
    ideName: "VS Code",
    isSubagent: true,
    parentSessionId: "sess-fleet-main",
    agentType: "explore",
    agentDescription: "Explore project structure",
    slug: null,
    aiTitle: null,
    status: "thinking",
    tokenSpeed: 8.7,
    totalOutputTokens: 12340,
    lastMessagePreview: "Searching for component files...",
    lastActivityMs: NOW - 2000,
    createdAtMs: NOW - 10 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/claude-fleet/sess-fleet-explore.jsonl",
    model: "claude-sonnet-4-20250514",
    thinkingLevel: "medium",
    pid: 12346,
    pidPrecise: true,
    lastSkill: null,
    agentSource: "claude-code",
    lastOutcome: null,
  },
  // Subagent: General-purpose agent for claude-fleet
  {
    id: "sess-fleet-gp",
    workspacePath: "/Users/demo/workspace/claude-fleet",
    workspaceName: "claude-fleet",
    ideName: "VS Code",
    isSubagent: true,
    parentSessionId: "sess-fleet-main",
    agentType: "general-purpose",
    agentDescription: "Write mock data module",
    slug: null,
    aiTitle: null,
    status: "executing",
    tokenSpeed: 15.2,
    totalOutputTokens: 8930,
    lastMessagePreview: "Writing src/mock/data.ts...",
    lastActivityMs: NOW - 1000,
    createdAtMs: NOW - 8 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/claude-fleet/sess-fleet-gp.jsonl",
    model: "claude-opus-4-20250805",
    thinkingLevel: null,
    pid: 12347,
    pidPrecise: true,
    lastSkill: null,
    agentSource: "claude-code",
    lastOutcome: null,
  },

  // ── 2. Active main session: "api-server" — executing ──
  {
    id: "sess-api-main",
    workspacePath: "/Users/demo/workspace/api-server",
    workspaceName: "api-server",
    ideName: "JetBrains",
    isSubagent: false,
    parentSessionId: null,
    agentType: null,
    agentDescription: null,
    slug: "fix/auth-middleware",
    aiTitle: "Fix JWT token validation in auth middleware",
    status: "executing",
    tokenSpeed: 22.5,
    totalOutputTokens: 95240,
    lastMessagePreview: "Running test suite after applying the fix...",
    lastActivityMs: NOW - 3000,
    createdAtMs: NOW - 45 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/api-server/sess-api-main.jsonl",
    model: "claude-opus-4-20250805",
    thinkingLevel: null,
    pid: 23456,
    pidPrecise: true,
    lastSkill: "/commit",
    agentSource: "claude-code",
    lastOutcome: ["bug_fixed"],
  },
  // Subagent: Plan agent for api-server
  {
    id: "sess-api-plan",
    workspacePath: "/Users/demo/workspace/api-server",
    workspaceName: "api-server",
    ideName: "JetBrains",
    isSubagent: true,
    parentSessionId: "sess-api-main",
    agentType: "plan",
    agentDescription: "Design auth middleware refactor",
    slug: null,
    aiTitle: null,
    status: "idle",
    tokenSpeed: 0,
    totalOutputTokens: 5620,
    lastMessagePreview: "Plan complete. Recommended approach: ...",
    lastActivityMs: NOW - 20 * MIN,
    createdAtMs: NOW - 30 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/api-server/sess-api-plan.jsonl",
    model: "claude-sonnet-4-20250514",
    thinkingLevel: "medium",
    pid: null,
    pidPrecise: false,
    lastSkill: null,
    agentSource: "claude-code",
    lastOutcome: null,
  },

  // ── 3. Cursor session: "mobile-app" — streaming ──
  {
    id: "sess-mobile-cursor",
    workspacePath: "/Users/demo/workspace/mobile-app",
    workspaceName: "mobile-app",
    ideName: "Cursor",
    isSubagent: false,
    parentSessionId: null,
    agentType: null,
    agentDescription: null,
    slug: null,
    aiTitle: "Add dark mode toggle to settings screen",
    status: "streaming",
    tokenSpeed: 35.8,
    totalOutputTokens: 67890,
    lastMessagePreview: "Implementing the ThemeProvider wrapper...",
    lastActivityMs: NOW - 500,
    createdAtMs: NOW - 15 * MIN,
    jsonlPath: "/Users/demo/.cursor/agent-transcripts/sess-mobile-cursor.jsonl",
    model: "claude-sonnet-4-20250514",
    thinkingLevel: null,
    pid: 34567,
    pidPrecise: false,
    lastSkill: null,
    agentSource: "cursor",
    lastOutcome: ["feature_added"],
  },

  // ── 4. Codex session: "data-pipeline" — processing ──
  {
    id: "sess-codex-pipeline",
    workspacePath: "/Users/demo/workspace/data-pipeline",
    workspaceName: "data-pipeline",
    ideName: "Terminal",
    isSubagent: false,
    parentSessionId: null,
    agentType: null,
    agentDescription: null,
    slug: null,
    aiTitle: "Optimize Spark job partitioning strategy",
    status: "processing",
    tokenSpeed: 18.4,
    totalOutputTokens: 34560,
    lastMessagePreview: "Analyzing current partition distribution...",
    lastActivityMs: NOW - 8000,
    createdAtMs: NOW - 35 * MIN,
    jsonlPath: "/Users/demo/.codex/sessions/sess-codex-pipeline.jsonl",
    model: "o3",
    thinkingLevel: "high",
    pid: 45678,
    pidPrecise: true,
    lastSkill: null,
    agentSource: "codex",
    lastOutcome: null,
  },

  // ── 5. Waiting for input: "web-frontend" ──
  {
    id: "sess-web-waiting",
    workspacePath: "/Users/demo/workspace/web-frontend",
    workspaceName: "web-frontend",
    ideName: "VS Code",
    isSubagent: false,
    parentSessionId: null,
    agentType: null,
    agentDescription: null,
    slug: "refactor/components",
    aiTitle: "Refactor shared components into design system",
    status: "waitingInput",
    tokenSpeed: 0,
    totalOutputTokens: 123400,
    lastMessagePreview: "Should I proceed with breaking changes to the Button component API?",
    lastActivityMs: NOW - 3 * MIN,
    createdAtMs: NOW - 1 * HOUR,
    jsonlPath: "/Users/demo/.claude/projects/web-frontend/sess-web-waiting.jsonl",
    model: "claude-opus-4-20250805",
    thinkingLevel: null,
    pid: 56789,
    pidPrecise: true,
    lastSkill: "/review-pr",
    agentSource: "claude-code",
    lastOutcome: ["needs_input"],
  },

  // ── 6. OpenClaw session: "ml-training" — active ──
  {
    id: "sess-openclaw-ml",
    workspacePath: "/Users/demo/workspace/ml-training",
    workspaceName: "ml-training",
    ideName: "Terminal",
    isSubagent: false,
    parentSessionId: null,
    agentType: null,
    agentDescription: null,
    slug: null,
    aiTitle: "Fine-tune classification model hyperparameters",
    status: "thinking",
    tokenSpeed: 9.1,
    totalOutputTokens: 41200,
    lastMessagePreview: "Evaluating learning rate schedules...",
    lastActivityMs: NOW - 12000,
    createdAtMs: NOW - 50 * MIN,
    jsonlPath: "/Users/demo/.openclaw/sessions/sess-openclaw-ml.jsonl",
    model: "claude-opus-4-20250805",
    thinkingLevel: null,
    pid: 67890,
    pidPrecise: true,
    lastSkill: null,
    agentSource: "openclaw",
    lastOutcome: null,
  },

  // ── 7. Idle session: "docs-site" ──
  {
    id: "sess-docs-idle",
    workspacePath: "/Users/demo/workspace/docs-site",
    workspaceName: "docs-site",
    ideName: "VS Code",
    isSubagent: false,
    parentSessionId: null,
    agentType: null,
    agentDescription: null,
    slug: "update/api-docs",
    aiTitle: "Update API documentation for v2 endpoints",
    status: "idle",
    tokenSpeed: 0,
    totalOutputTokens: 78900,
    lastMessagePreview: "Documentation updated successfully. All links verified.",
    lastActivityMs: NOW - 2 * HOUR,
    createdAtMs: NOW - 3 * HOUR,
    jsonlPath: "/Users/demo/.claude/projects/docs-site/sess-docs-idle.jsonl",
    model: "claude-sonnet-4-20250514",
    thinkingLevel: "medium",
    pid: null,
    pidPrecise: false,
    lastSkill: null,
    agentSource: "claude-code",
    lastOutcome: ["celebrating"],
  },

  // ── 8. Idle session: "infra-terraform" ──
  {
    id: "sess-infra-idle",
    workspacePath: "/Users/demo/workspace/infra-terraform",
    workspaceName: "infra-terraform",
    ideName: "JetBrains",
    isSubagent: false,
    parentSessionId: null,
    agentType: null,
    agentDescription: null,
    slug: "feat/auto-scaling",
    aiTitle: "Configure auto-scaling policies for ECS services",
    status: "idle",
    tokenSpeed: 0,
    totalOutputTokens: 56000,
    lastMessagePreview: "Terraform plan looks good. Ready to apply.",
    lastActivityMs: NOW - 4 * HOUR,
    createdAtMs: NOW - 5 * HOUR,
    jsonlPath: "/Users/demo/.claude/projects/infra-terraform/sess-infra-idle.jsonl",
    model: "claude-opus-4-20250805",
    thinkingLevel: null,
    pid: null,
    pidPrecise: false,
    lastSkill: "/commit",
    agentSource: "claude-code",
    lastOutcome: ["feature_added"],
  },
];

// ── Messages (for session detail view) ──────────────────────────────────────

export const MOCK_MESSAGES: Record<string, RawMessage[]> = {
  // Main fleet session — rich conversation
  "sess-fleet-main": [
    {
      type: "user",
      uuid: "msg-1",
      timestamp: new Date(NOW - 25 * MIN).toISOString(),
      message: {
        role: "user",
        content: "I need to create a mock mode for this Tauri desktop app so we can take screenshots for the README. The mock should show realistic session data with multiple agent types (Claude Code, Cursor, Codex, OpenClaw) in various states.",
      },
    },
    {
      type: "assistant",
      uuid: "msg-2",
      timestamp: new Date(NOW - 24 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-20250805",
        content: [
          {
            type: "thinking",
            thinking: "The user wants to create a mock/demo mode for their Tauri app. I need to:\n1. Understand the data flow - what Tauri IPC commands the frontend calls\n2. Create mock data that covers all session types and statuses\n3. Intercept Tauri API calls in the frontend\n\nLet me first explore the codebase to understand the invoke commands used...",
          },
          {
            type: "text",
            text: "I'll create a mock mode that intercepts Tauri IPC calls and returns realistic demo data. Let me first explore the codebase to understand all the data flows.",
          },
        ],
        stop_reason: "end_turn",
        usage: { input_tokens: 1250, output_tokens: 340 },
      },
    },
    {
      type: "assistant",
      uuid: "msg-3",
      timestamp: new Date(NOW - 23 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-20250805",
        content: [
          {
            type: "tool_use",
            id: "tool-1",
            name: "Grep",
            input: { pattern: "invoke<", glob: "src/**/*.{ts,tsx}", output_mode: "content" },
          },
        ],
        stop_reason: "tool_use",
        usage: { input_tokens: 2100, output_tokens: 120 },
      },
    },
    {
      type: "assistant",
      uuid: "msg-3b",
      timestamp: new Date(NOW - 23 * MIN).toISOString(),
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "tool-1",
            content: `src/store.ts:90:    const sessions = await invoke<SessionInfo[]>("list_sessions");
src/store.ts:118:    const rawMessages = await invoke<RawMessage[]>("get_messages", {
src/components/AccountInfo.tsx:104:      const data = await invoke<AccountInfoData>("get_account_info");
src/components/UsagePanel.tsx:259:      const data = await invoke<AccountInfoData>("get_account_info");
src/components/MemoryPanel.tsx:77:      const data = await invoke<WorkspaceMemory[]>("list_memories");`,
          },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "msg-4",
      timestamp: new Date(NOW - 20 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-20250805",
        content: [
          {
            type: "text",
            text: "I've identified all the Tauri IPC calls. Now let me create the mock data module with sessions covering:\n\n- **Claude Code**: Main sessions + subagents (explore, plan, general-purpose)\n- **Cursor**: Streaming session\n- **Codex**: Processing session\n- **OpenClaw**: Thinking session\n- Various statuses: thinking, executing, streaming, processing, waitingInput, delegating, idle\n\nI'll also mock the usage data, memory panel, and account info.",
          },
          {
            type: "tool_use",
            id: "tool-2",
            name: "Write",
            input: { file_path: "/Users/demo/workspace/claude-fleet/src/mock/data.ts", content: "// Mock data..." },
          },
        ],
        stop_reason: "tool_use",
        usage: { input_tokens: 3500, output_tokens: 890 },
      },
    },
    {
      type: "assistant",
      uuid: "msg-4b",
      timestamp: new Date(NOW - 20 * MIN).toISOString(),
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "tool-2",
            content: "File written successfully.",
          },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "msg-5",
      timestamp: new Date(NOW - 15 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-20250805",
        content: [
          {
            type: "text",
            text: "Mock data module created. Now creating the Tauri API mock layer that intercepts `invoke()` and `listen()` calls.\n\n```typescript\n// src/mock/tauri-mock.ts\nconst handlers: Record<string, Function> = {\n  list_sessions: () => MOCK_SESSIONS,\n  get_messages: ({ jsonlPath }) => MOCK_MESSAGES[id],\n  get_account_info: () => MOCK_ACCOUNT,\n  // ...\n};\n```\n\nThis will allow the app to run in any browser without the Tauri runtime.",
          },
        ],
        stop_reason: "end_turn",
        usage: { input_tokens: 4200, output_tokens: 450 },
      },
    },
  ],

  // API server session — shorter
  "sess-api-main": [
    {
      type: "user",
      uuid: "api-msg-1",
      timestamp: new Date(NOW - 45 * MIN).toISOString(),
      message: {
        role: "user",
        content: "The JWT validation in our auth middleware is rejecting valid tokens after the recent update. Users are getting 401 errors. Can you investigate and fix it?",
      },
    },
    {
      type: "assistant",
      uuid: "api-msg-2",
      timestamp: new Date(NOW - 44 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-20250805",
        content: [
          {
            type: "thinking",
            thinking: "The JWT validation issue is likely caused by the recent update changing how the `iss` (issuer) claim is validated. Let me check the middleware code and the JWT library version...",
          },
          {
            type: "text",
            text: "I'll investigate the auth middleware. Let me check the JWT validation logic and the recent changes.",
          },
          {
            type: "tool_use",
            id: "api-tool-1",
            name: "Read",
            input: { file_path: "/Users/demo/workspace/api-server/src/middleware/auth.ts" },
          },
        ],
        stop_reason: "tool_use",
        usage: { input_tokens: 1800, output_tokens: 280 },
      },
    },
    {
      type: "assistant",
      uuid: "api-msg-2b",
      timestamp: new Date(NOW - 44 * MIN).toISOString(),
      message: {
        role: "user",
        content: [
          {
            type: "tool_result",
            tool_use_id: "api-tool-1",
            content: `export async function validateToken(token: string): Promise<JWTPayload> {
  const decoded = jwt.verify(token, process.env.JWT_SECRET!, {
    algorithms: ['HS256'],
    issuer: 'api-server-v2',  // ← Changed from 'api-server'
    audience: 'web-client',
  });
  return decoded as JWTPayload;
}`,
          },
        ],
      },
    },
    {
      type: "assistant",
      uuid: "api-msg-3",
      timestamp: new Date(NOW - 40 * MIN).toISOString(),
      message: {
        role: "assistant",
        model: "claude-opus-4-20250805",
        content: [
          {
            type: "text",
            text: "Found it. The issuer claim was changed from `'api-server'` to `'api-server-v2'`, but existing tokens still have the old issuer. The fix is to accept both issuers during the migration period.\n\n```typescript\nissuer: ['api-server', 'api-server-v2'],  // Accept both during migration\n```\n\nApplying the fix and running tests now.",
          },
        ],
        stop_reason: "end_turn",
        usage: { input_tokens: 3200, output_tokens: 320 },
      },
    },
  ],
};

// Provide a default set of messages for any session not explicitly mocked
export function getMessagesForSession(sessionId: string): RawMessage[] {
  if (MOCK_MESSAGES[sessionId]) return MOCK_MESSAGES[sessionId];
  // Generate generic messages
  const session = MOCK_SESSIONS.find((s) => s.id === sessionId);
  if (!session) return [];
  return [
    {
      type: "user",
      uuid: `${sessionId}-msg-1`,
      timestamp: new Date(session.createdAtMs).toISOString(),
      message: {
        role: "user",
        content: session.aiTitle ?? "Help me with this task.",
      },
    },
    {
      type: "assistant",
      uuid: `${sessionId}-msg-2`,
      timestamp: new Date(session.createdAtMs + 30000).toISOString(),
      message: {
        role: "assistant",
        model: session.model ?? "claude-opus-4-20250805",
        content: [
          {
            type: "thinking",
            thinking: "Let me analyze the request and determine the best approach...",
          },
          {
            type: "text",
            text: session.lastMessagePreview ?? "I'm working on this task now.",
          },
        ],
        stop_reason: session.status === "idle" ? "end_turn" : null,
        usage: { input_tokens: 1500, output_tokens: 200 },
      },
    },
  ];
}

// ── Waiting alerts ──────────────────────────────────────────────────────────

export const MOCK_WAITING_ALERTS: WaitingAlert[] = [
  {
    sessionId: "sess-web-waiting",
    workspaceName: "web-frontend",
    summary: "Should I proceed with breaking changes to the Button component API?",
    detectedAtMs: NOW - 3 * MIN,
    jsonlPath: "/Users/demo/.claude/projects/web-frontend/sess-web-waiting.jsonl",
  },
];

// ── Skill history ───────────────────────────────────────────────────────────

export const MOCK_SKILL_HISTORY: Record<string, SkillInvocation[]> = {
  "sess-api-main": [
    { skill: "review-pr", args: "142", timestamp: new Date(NOW - 42 * MIN).toISOString() },
    { skill: "commit", args: "-m 'Fix JWT issuer validation'", timestamp: new Date(NOW - 10 * MIN).toISOString() },
  ],
  "sess-web-waiting": [
    { skill: "review-pr", args: "87", timestamp: new Date(NOW - 55 * MIN).toISOString() },
  ],
};

// ── Account / usage info ────────────────────────────────────────────────────

export const MOCK_ACCOUNT_INFO = {
  email: "developer@example.com",
  full_name: "Alex Chen",
  organization_name: "Acme Corp",
  plan: "max_5x",
  auth_method: "api_key",
  five_hour: {
    utilization: 0.42,
    resets_at: new Date(NOW + 2.5 * HOUR).toISOString(),
    prev_utilization: 0.38,
  },
  seven_day: {
    utilization: 0.28,
    resets_at: new Date(NOW + 3 * 24 * HOUR).toISOString(),
    prev_utilization: 0.31,
  },
  seven_day_sonnet: {
    utilization: 0.15,
    resets_at: new Date(NOW + 3 * 24 * HOUR).toISOString(),
    prev_utilization: 0.12,
  },
};

export const MOCK_CURSOR_USAGE = {
  email: "developer@example.com",
  signUpType: "email",
  membershipType: "pro",
  subscriptionStatus: "active",
  totalPrompts: 1847,
  dailyStats: [],
  usage: [
    { name: "Prompts", used: 347, limit: 500, utilization: 0.694, resetsAt: new Date(NOW + 18 * HOUR).toISOString() },
    { name: "Fast Agent Credits", used: 12, limit: 50, utilization: 0.24, resetsAt: new Date(NOW + 18 * HOUR).toISOString() },
  ],
};

export const MOCK_CODEX_USAGE = {
  limitId: "codex-standard",
  limitName: "Standard",
  planType: "pro",
  primary: { usedPercent: 35, windowDurationMins: 300, resetsAt: NOW + 2 * HOUR },
  secondary: { usedPercent: 18, windowDurationMins: 10080, resetsAt: NOW + 4 * 24 * HOUR },
  credits: { hasCredits: true, unlimited: false, balance: "$42.50" },
};

export const MOCK_OPENCLAW_USAGE = {
  sessions: [
    { sessionId: "sess-openclaw-ml", contextPercent: 62, maxContext: 200000, usedContext: 124000 },
  ],
};

export const MOCK_OPENCLAW_ACCOUNT = {
  version: "0.4.2",
  defaultModel: "claude-opus-4-20250805",
  providers: [
    { provider: "anthropic", authType: "api_key", status: "active", label: "Anthropic Direct", expiresAt: null, remainingMs: null },
    { provider: "openrouter", authType: "api_key", status: "active", label: "OpenRouter", expiresAt: null, remainingMs: null },
  ],
};

// ── Memory panel ────────────────────────────────────────────────────────────

export const MOCK_MEMORIES = [
  {
    workspaceName: "claude-fleet",
    workspacePath: "/Users/demo/workspace/claude-fleet",
    projectKey: "claude-fleet-key",
    hasClaudeMd: true,
    files: [
      { name: "MEMORY.md", path: "/Users/demo/.claude/projects/claude-fleet/memory/MEMORY.md", sizeBytes: 1240, modifiedMs: NOW - 2 * HOUR },
      { name: "feedback_backend_sync.md", path: "/Users/demo/.claude/projects/claude-fleet/memory/feedback_backend_sync.md", sizeBytes: 890, modifiedMs: NOW - 24 * HOUR },
    ],
  },
  {
    workspaceName: "api-server",
    workspacePath: "/Users/demo/workspace/api-server",
    projectKey: "api-server-key",
    hasClaudeMd: true,
    files: [
      { name: "MEMORY.md", path: "/Users/demo/.claude/projects/api-server/memory/MEMORY.md", sizeBytes: 2100, modifiedMs: NOW - 5 * HOUR },
    ],
  },
  {
    workspaceName: "web-frontend",
    workspacePath: "/Users/demo/workspace/web-frontend",
    projectKey: "web-frontend-key",
    hasClaudeMd: false,
    files: [
      { name: "MEMORY.md", path: "/Users/demo/.claude/projects/web-frontend/memory/MEMORY.md", sizeBytes: 640, modifiedMs: NOW - 12 * HOUR },
    ],
  },
];

export const MOCK_MEMORY_CONTENT = `---
name: feedback_backend_sync
description: All new features must implement both LocalBackend and RemoteBackend
type: feedback
---

All new features must implement both LocalBackend and RemoteBackend (plus fleet serve probe endpoints). Never bypass the Backend trait.

**Why:** Prior incident where a feature only worked locally and broke remote monitoring for all users.
**How to apply:** When adding any new IPC command or data source, always implement it in both backend modules.`;

export const MOCK_MEMORY_HISTORY = [
  {
    sessionId: "sess-fleet-main",
    workspaceName: "claude-fleet",
    timestamp: new Date(NOW - 24 * HOUR).toISOString(),
    tool: "Write",
    detail: {
      type: "write" as const,
      content: "---\nname: feedback_backend_sync\n...",
    },
  },
];

// ── Sources config ──────────────────────────────────────────────────────────

export const MOCK_SOURCES_CONFIG = [
  { name: "claude-code", enabled: true, available: true },
  { name: "cursor", enabled: true, available: true },
  { name: "openclaw", enabled: true, available: true },
  { name: "codex", enabled: true, available: true },
];

// ── Setup status ────────────────────────────────────────────────────────────

export const MOCK_SETUP_STATUS = {
  cli_installed: true,
  claude_dir_exists: true,
  has_sessions: true,
  detected_tools: { openclaw: true, cursor: true, codex: true },
};

// ── Hooks setup plan ────────────────────────────────────────────────────────

export const MOCK_HOOKS_PLAN = {
  toAdd: [],
  hooksGloballyDisabled: false,
  alreadyInstalled: true,
};

// ── AI tool detection ───────────────────────────────────────────────────────

export const MOCK_DETECTED_TOOLS = [
  { name: "Claude Code", path: "/usr/local/bin/claude", version: "1.0.33" },
  { name: "Cursor", path: "/Applications/Cursor.app", version: "0.50.1" },
  { name: "OpenClaw", path: "/usr/local/bin/openclaw", version: "0.4.2" },
  { name: "Codex", path: "/usr/local/bin/codex", version: "0.1.5" },
];
