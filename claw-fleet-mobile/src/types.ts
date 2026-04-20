// Shared types — mirrored from Desktop's src/types.ts

export type SessionStatus =
  | "thinking"
  | "executing"
  | "streaming"
  | "processing"
  | "waitingInput"
  | "active"
  | "delegating"
  | "idle";

export interface SessionInfo {
  id: string;
  workspacePath: string;
  workspaceName: string;
  ideName: string | null;
  isSubagent: boolean;
  parentSessionId: string | null;
  agentType: string | null;
  agentDescription: string | null;
  slug: string | null;
  aiTitle: string | null;
  status: SessionStatus;
  tokenSpeed: number;
  totalOutputTokens: number;
  totalCostUsd: number;
  agentTotalCostUsd: number;
  costSpeedUsdPerMin: number;
  lastMessagePreview: string | null;
  lastActivityMs: number;
  createdAtMs: number;
  jsonlPath: string;
  model: string | null;
  thinkingLevel: string | null;
  pid: number | null;
  pidPrecise: boolean;
  lastSkill: string | null;
  contextPercent: number | null;
  agentSource: "claude-code" | "cursor" | "openclaw" | "codex";
}

export interface WaitingAlert {
  sessionId: string;
  workspaceName: string;
  summary: string;
  detectedAtMs: number;
  jsonlPath: string;
  source: string;
}

export type ContentBlockType =
  | "text"
  | "tool_use"
  | "tool_result"
  | "thinking"
  | "redacted_thinking"
  | "image";

export interface ContentBlock {
  type: string;
  [key: string]: unknown;
}

export interface RawMessage {
  type: string;
  uuid?: string;
  timestamp?: string;
  message?: {
    role: "user" | "assistant";
    model?: string;
    content: ContentBlock[] | string;
    stop_reason?: string | null;
    usage?: {
      input_tokens: number;
      output_tokens: number;
    };
  };
}

// ── Guard types ─────────────────────────────────────────────────────────────

export interface GuardRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  toolName: string;
  command: string;
  commandSummary: string;
  riskTags: string[];
  timestamp: string;
}

export interface GuardDecision {
  kind: "guard";
  id: string;
  request: GuardRequest;
  analysis: string | null;
  analyzing: boolean;
  arrivedAt: number;
}

// ── Elicitation types ──────────────────────────────────────────────────

export interface ElicitationOption {
  label: string;
  description: string;
}

export interface ElicitationQuestion {
  question: string;
  header: string;
  options: ElicitationOption[];
  multiSelect: boolean;
}

export interface ElicitationRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  questions: ElicitationQuestion[];
  timestamp: string;
}

export interface ElicitationAttachment {
  /** Absolute path the agent will see (already uploaded to the server). */
  path: string;
  /** Display name (basename of the original file). */
  name: string;
  /** true when the attachment came from clipboard/paste. */
  fromClipboard?: boolean;
}

export interface ElicitationDecision {
  kind: "elicitation";
  id: string;
  request: ElicitationRequest;
  step: number;
  selections: Record<string, string[]>;
  customAnswers: Record<string, string>;
  /** User-forced multi-select per question (only populated when user flips a single-select to multi). */
  multiSelectOverrides: Record<string, boolean>;
  /** Per-question attachments (question text → attachments). */
  attachments: Record<string, ElicitationAttachment[]>;
  arrivedAt: number;
}

export type PendingDecision = GuardDecision | ElicitationDecision;

// ── Audit types ─────────────────────────────────────────────────────────────

export type AuditRiskLevel = "medium" | "high" | "critical";

export interface AuditEvent {
  sessionId: string;
  workspaceName: string;
  agentSource: string;
  toolName: string;
  commandSummary: string;
  fullCommand: string;
  riskLevel: AuditRiskLevel;
  riskTags: string[];
  timestamp: string;
  jsonlPath: string;
}

export interface AuditSummary {
  events: AuditEvent[];
  totalSessionsScanned: number;
}

export interface Lesson {
  content: string;
  reason: string;
  workspaceName: string;
  sessionId: string;
}

export interface DailyReport {
  date: string;
  timezone: string;
  generatedAt: number;
  metrics: DailyMetrics;
  aiSummary: string | null;
  sessionIds: string[];
  lessons: Lesson[] | null;
}

export interface DailyMetrics {
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheCreationTokens?: number;
  totalCacheReadTokens?: number;
  totalWebSearchRequests?: number;
  totalCostUsd?: number;
  totalSessions: number;
  totalSubagents: number;
  totalToolCalls: number;
  toolCallBreakdown: Record<string, number>;
  modelBreakdown: Record<string, { inputTokens: number; outputTokens: number; costUsd?: number }>;
  projects: ProjectMetrics[];
  hourlyActivity: number[];
}

export interface ProjectMetrics {
  workspacePath: string;
  workspaceName: string;
  sessionCount: number;
  subagentCount: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  totalCacheCreationTokens?: number;
  totalCacheReadTokens?: number;
  totalWebSearchRequests?: number;
  totalCostUsd?: number;
  toolCalls: number;
}

export interface DailyReportStats {
  date: string;
  totalTokens: number;
  totalSessions: number;
  totalToolCalls: number;
  totalProjects: number;
}
