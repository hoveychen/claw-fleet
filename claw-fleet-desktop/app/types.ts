// ── Session types (mirroring Rust structs) ───────────────────────────────────

export type SessionStatus =
  | "thinking"
  | "executing"
  | "streaming"
  | "processing"
  | "waitingInput"
  | "active"
  | "delegating"
  | "idle"
  | "rateLimited";

export type RateLimitType =
  | "sessionLimit"
  | "weeklyLimit"
  | "opusLimit"
  | "sonnetLimit"
  | "usageLimit"
  | "outOfExtraUsage"
  | "unknown";

export interface RateLimitState {
  resetsAt: string; // ISO-8601 UTC
  limitType: RateLimitType;
  parsed: boolean;
  errorTimestamp: string; // ISO-8601 UTC
}

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
  lastOutcome: SessionOutcome[] | null;
  rateLimit?: RateLimitState | null;
  /** Snapshot of the latest TodoWrite state; absent when the session has never invoked TodoWrite. */
  todos?: TodoSummary | null;
  /** Number of times this session was context-compacted (auto or manual /compact). */
  compactCount?: number;
  /** Sum of context sizes (in tokens) right before each compaction. */
  compactPreTokens?: number;
  /** Sum of summary sizes (in tokens) produced by each compaction. */
  compactPostTokens?: number;
  /** Estimated USD cost of compact LLM calls (the calls themselves are not
   *  recorded as standalone turns, so this is an approximation). */
  compactCostUsd?: number;
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
  /** Originating agent source — e.g. "claude-code", "cursor", "codex". */
  source: string;
}

export interface SkillInvocation {
  skill: string;
  args: string | null;
  timestamp: string;
  isSubagent: boolean;
}

export type SessionTodoStatus = "pending" | "in_progress" | "completed";

export interface SessionTodo {
  content: string;
  activeForm: string;
  status: SessionTodoStatus;
}

export interface TodoSummary {
  completed: number;
  inProgress: number;
  pending: number;
  /** activeForm of the first in-progress todo; absent when nothing is in progress. */
  currentActive?: string;
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
  isCompactSummary?: boolean;
  isVisibleInTranscriptOnly?: boolean;
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

export interface AuditRuleInfo {
  id: string;
  level: AuditRiskLevel;
  tag: string;
  matchMode: "contains" | "command_start";
  patterns: string[];
  descriptionEn: string;
  descriptionZh: string;
  enabled: boolean;
  builtin: boolean;
  category: string;
}

export interface SuggestedRule {
  id: string;
  level: AuditRiskLevel;
  tag: string;
  matchMode: "contains" | "command_start";
  patterns: string[];
  descriptionEn: string;
  descriptionZh: string;
  category: string;
  reasoning: string;
}

// ── Guard types ─────────────────────────────────────────────────────────────

export interface GuardRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  /** AI-generated session title (separate from workspaceName). */
  aiTitle?: string | null;
  toolName: string;
  command: string;
  commandSummary: string;
  riskTags: string[];
  timestamp: string;
}

// ── Decision panel types (abstract, extensible) ────────────────────────────

/** Guard interception decision — user must allow or block a critical command. */
export interface GuardDecision {
  kind: "guard";
  id: string;
  request: GuardRequest;
  analysis: string | null;
  analyzing: boolean;
  arrivedAt: number; // epoch ms
}

// ── Elicitation types ──────────────────────────────────────────────────

export interface ElicitationOption {
  label: string;
  description: string;
  preview?: string;
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
  /** AI-generated session title (separate from workspaceName). */
  aiTitle?: string | null;
  questions: ElicitationQuestion[];
  timestamp: string;
}

/** A file/image the user attached to a decision-panel answer. */
export interface ElicitationAttachment {
  /** Absolute path the agent will see (already uploaded for RemoteBackend). */
  path: string;
  /** Display name (basename of the original file). */
  name: string;
  /** true when saved from clipboard paste. */
  fromClipboard?: boolean;
  /** In-memory blob URL for thumbnail preview (image attachments only). */
  previewUrl?: string;
  /** Natural image width, if the attachment is a decoded image. */
  width?: number;
  /** Natural image height, if the attachment is a decoded image. */
  height?: number;
}

/** Agent is asking the user a question via AskUserQuestion. */
export interface ElicitationDecision {
  kind: "elicitation";
  id: string;
  request: ElicitationRequest;
  /** Current step index (0-based). */
  step: number;
  /** Current selections: question text → selected option label(s). */
  selections: Record<string, string[]>;
  /** Custom "Other" text per question: question text → user-typed string. */
  customAnswers: Record<string, string>;
  /**
   * User-forced multi-select per question: question text → true if user
   * flipped a single-select question into multi-select. Undefined/false means
   * use `question.multiSelect` as-is.
   */
  multiSelectOverrides: Record<string, boolean>;
  /** Per-question attachment list (question text → attachments). */
  attachments: Record<string, ElicitationAttachment[]>;
  arrivedAt: number;
}

// ── Plan approval types ────────────────────────────────────────────────

export interface PlanApprovalRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  /** AI-generated session title (separate from workspaceName). */
  aiTitle?: string | null;
  planContent: string;
  planFilePath?: string | null;
  timestamp: string;
}

/** Agent is asking the user to approve/reject an ExitPlanMode plan. */
export interface PlanApprovalDecision {
  kind: "plan-approval";
  id: string;
  request: PlanApprovalRequest;
  /** User's edited version of the plan content; null = unchanged. */
  editedPlan: string | null;
  /** Optional feedback/edits note to send when rejecting. */
  feedback: string;
  arrivedAt: number;
}

// ── Decision history (persisted log for `list_session_decisions`) ──────

export type ElicitationOutcome =
  | "answered"
  | "declined"
  | "heartbeat-lost"
  | "timeout";

export type PlanApprovalOutcome =
  | "approved"
  | "approved-with-edits"
  | "rejected"
  | "heartbeat-lost"
  | "timeout";

export interface SelectedOption {
  label: string;
  description?: string | null;
  /** True when the user typed via the "Other" escape hatch. */
  other?: boolean;
}

export interface ElicitationHistoryRecord {
  kind: "elicitation";
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  requestedAt: string;
  resolvedAt: string;
  outcome: ElicitationOutcome;
  questions: ElicitationQuestion[];
  /** question text → selected option (empty unless outcome === "answered"). */
  answers: Record<string, SelectedOption>;
}

export interface PlanApprovalHistoryRecord {
  kind: "plan-approval";
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  requestedAt: string;
  resolvedAt: string;
  outcome: PlanApprovalOutcome;
  planContent: string;
  planFilePath?: string | null;
  editedPlan?: string | null;
  feedback?: string | null;
}

export interface UserPromptHistoryRecord {
  kind: "user-prompt";
  id: string;
  sessionId: string;
  text: string;
  hasImage?: boolean;
  sentAt: string;
}

export type DecisionHistoryRecord =
  | ElicitationHistoryRecord
  | PlanApprovalHistoryRecord
  | UserPromptHistoryRecord;

/** Union of all decision types the panel can display. */
export type PendingDecision =
  | GuardDecision
  | ElicitationDecision
  | PlanApprovalDecision;

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
  totalCacheCreationTokens?: number;
  totalCacheReadTokens?: number;
  totalWebSearchRequests?: number;
  totalCostUsd?: number;
  totalSessions: number;
  totalSubagents: number;
  totalToolCalls: number;
  toolCallBreakdown: Record<string, number>;
  modelBreakdown: Record<
    string,
    {
      inputTokens: number;
      outputTokens: number;
      cacheCreationTokens?: number;
      cacheReadTokens?: number;
      costUsd?: number;
    }
  >;
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
  totalCacheCreationTokens?: number;
  totalCacheReadTokens?: number;
  totalWebSearchRequests?: number;
  totalCostUsd?: number;
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
  costUsd?: number;
  agentSource: string;
}

export interface DailyReportStats {
  date: string;
  totalTokens: number;
  totalSessions: number;
  totalToolCalls: number;
  totalProjects: number;
}
