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
