// ── Session types (mirroring Rust structs) ───────────────────────────────────

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
  lastOutcome: SessionOutcome[] | null;
}

export type SessionOutcome =
  | "needs_input"
  | "bug_fixed"
  | "feature_added"
  | "stuck"
  | "apologizing"
  | "show_off"
  | "concerned"
  | "confused"
  | "celebrating"
  | "quick_fix"
  | "overwhelmed"
  | "scheming"
  | "reporting";

export interface SearchHit {
  sessionId: string;
  jsonlPath: string;
  snippet: string;
  rank: number;
}

export interface WaitingAlert {
  sessionId: string;
  workspaceName: string;
  summary: string;
  detectedAtMs: number;
  jsonlPath: string;
}

export interface SkillInvocation {
  skill: string;
  args: string | null;
  timestamp: string;
}

// ── Message / content block types ───────────────────────────────────────────

export type ContentBlockType =
  | "text"
  | "tool_use"
  | "tool_result"
  | "thinking"
  | "redacted_thinking"
  | "image"
  | "document"
  | "server_tool_use"
  | "web_search_tool_result"
  | "search_result";

export interface TextBlock {
  type: "text";
  text: string;
}

export interface ToolUseBlock {
  type: "tool_use";
  id: string;
  name: string;
  input: Record<string, unknown>;
}

export interface ToolResultBlock {
  type: "tool_result";
  tool_use_id: string;
  content: string | ContentBlock[];
  is_error?: boolean;
}

export interface ThinkingBlock {
  type: "thinking";
  thinking: string;
}

export interface RedactedThinkingBlock {
  type: "redacted_thinking";
}

export interface ImageBlock {
  type: "image";
  source: { type: string; media_type: string; data: string };
}

export type ContentBlock =
  | TextBlock
  | ToolUseBlock
  | ToolResultBlock
  | ThinkingBlock
  | RedactedThinkingBlock
  | ImageBlock
  | { type: string; [key: string]: unknown };

export interface MessageUsage {
  input_tokens: number;
  output_tokens: number;
  cache_creation_input_tokens?: number;
  cache_read_input_tokens?: number;
}

export interface RawMessage {
  type: "user" | "assistant" | "progress" | "queue-operation" | "last-prompt" | "file-history-snapshot";
  uuid?: string;
  timestamp?: string;
  isSidechain?: boolean;
  agentId?: string;
  sessionId?: string;
  slug?: string;
  message?: {
    role: "user" | "assistant";
    model?: string;
    id?: string;
    content: ContentBlock[] | string;
    stop_reason?: string | null;
    usage?: MessageUsage;
  };
}

// ── Security audit types ────────────────────────────────────────────────────

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

export interface AuditAlert {
  key: string;
  sessionId: string;
  workspaceName: string;
  commandSummary: string;
  riskTags: string[];
  detectedAtMs: number;
  jsonlPath: string;
}

// ── Daily Report types ────────────────────────────────────────────────────────

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
  aiSummaryGeneratedAt: number | null;
  sessionIds: string[];
  lessons: Lesson[] | null;
  lessonsGeneratedAt: number | null;
}

export interface DailyMetrics {
  totalInputTokens: number;
  totalOutputTokens: number;
  totalSessions: number;
  totalSubagents: number;
  totalToolCalls: number;
  toolCallBreakdown: Record<string, number>;
  modelBreakdown: Record<string, { inputTokens: number; outputTokens: number }>;
  projects: ProjectMetrics[];
  sourceBreakdown: Record<string, number>;
  hourlyActivity: number[];
}

export interface ProjectMetrics {
  workspacePath: string;
  workspaceName: string;
  sessionCount: number;
  subagentCount: number;
  totalInputTokens: number;
  totalOutputTokens: number;
  toolCalls: number;
  sessions: ReportSessionSummary[];
}

export interface ReportSessionSummary {
  id: string;
  title: string | null;
  lastMessage: string | null;
  model: string | null;
  isSubagent: boolean;
  outputTokens: number;
  agentSource: string;
}

export interface DailyReportStats {
  date: string;
  totalTokens: number;
  totalSessions: number;
  totalToolCalls: number;
  totalProjects: number;
}
