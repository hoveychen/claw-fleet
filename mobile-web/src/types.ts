// Mirrors of the desktop app's TypeScript types (claw-fleet-desktop/app/types.ts)
// for the payloads that cross the relay. Field names are the serde camelCase
// forms — keep in sync with the Rust structs.

// ── Decision requests ────────────────────────────────────────────────────────

/** Structured shell AST shipped in GuardRequest (claw-fleet-core/src/cmd_ast.rs).
 *  CommandLeaf has no serde rename_all — fields stay snake_case on the wire. */
export type CmdConnector = "and" | "or" | "pipe" | "semi";

export interface NestedScript {
  kind: string;
  raw: string;
  view: CommandView;
}

export interface CommandLeaf {
  argv: string[];
  nested?: NestedScript | null;
  triggering?: boolean;
  already_allowed?: boolean;
}

export interface CommandView {
  leaves: CommandLeaf[];
  connectors: CmdConnector[];
}

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
  structuredCommand?: CommandView | null;
}

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
  aiTitle?: string | null;
  questions: ElicitationQuestion[];
  timestamp: string;
}

export type FleetAskFormFieldKind =
  | "text"
  | "textarea"
  | "number"
  | "select"
  | "radio"
  | "checkbox"
  | "date"
  | "datetime"
  | "time"
  | "range";

export interface FleetAskFormField {
  name: string;
  kind: FleetAskFormFieldKind;
  label: string;
  placeholder?: string;
  options?: string[];
  required?: boolean;
  default?: unknown;
  min?: number;
  max?: number;
  step?: number;
}

export interface FleetAskImage {
  name: string;
  caption?: string;
}

export interface FleetAskQuestion {
  question: string;
  header: string;
  multiSelect: boolean;
  options?: ElicitationOption[];
  html?: string;
  formFields?: FleetAskFormField[];
  images?: FleetAskImage[];
}

export interface FleetAskRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  questions: FleetAskQuestion[];
  timestamp: string;
}

export interface PlanApprovalRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  planContent: string;
  planFilePath?: string | null;
  timestamp: string;
}

export interface PermissionPromptRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  timestamp: string;
  toolName: string;
  toolInput: unknown;
  toolUseId?: string | null;
}

/** fleet__render_a2ui request (claw-fleet-core/src/mcp_a2ui_ipc.rs). The
 *  message tree is opaque to the mobile client — it renders a placeholder
 *  card (desktop renders the real surface via @a2ui/react). */
export interface A2uiRenderRequest {
  id: string;
  sessionId: string;
  workspaceName: string;
  aiTitle?: string | null;
  timestamp: string;
  messageTree: unknown;
}

export type DecisionKind =
  | "guard"
  | "elicitation"
  | "fleet-ask"
  | "plan-approval"
  | "permission-prompt"
  | "a2ui-render";

export type DecisionRequest =
  | GuardRequest
  | ElicitationRequest
  | FleetAskRequest
  | PlanApprovalRequest
  | PermissionPromptRequest
  | A2uiRenderRequest;

export interface PendingDecision {
  kind: DecisionKind;
  id: string;
  request: DecisionRequest;
  arrivedAt: number;
}

/** `pending_snapshot` reply shape (see mobile_relay::serve_request). */
export interface PendingSnapshot {
  guard?: GuardRequest[];
  elicitation?: ElicitationRequest[];
  fleetAsk?: FleetAskRequest[];
  planApproval?: PlanApprovalRequest[];
  permissionPrompt?: PermissionPromptRequest[];
  a2uiRender?: A2uiRenderRequest[];
}

// ── Sessions / tasks ─────────────────────────────────────────────────────────

export type SessionStatus =
  | "thinking"
  | "executing"
  | "streaming"
  | "delegating"
  | "processing"
  | "waitingInput"
  | "active"
  | "idle"
  | "rateLimited";

export interface TodoSummary {
  completed: number;
  inProgress: number;
  pending: number;
  currentActive?: string | null;
}

export interface TaskPlanSummary {
  done: number;
  total: number;
  currentPlan?: string | null;
  planId?: string | null;
  currentTask?: string | null;
}

export type SessionMark = "pending" | "done";

export interface SessionInfo {
  id: string;
  workspacePath: string;
  workspaceName: string;
  aiTitle?: string | null;
  slug?: string | null;
  status: SessionStatus;
  isSubagent: boolean;
  lastMessagePreview?: string | null;
  lastActivityMs: number;
  createdAtMs: number;
  jsonlPath: string;
  model?: string | null;
  agentSource?: string;
  contextPercent?: number | null;
  totalCostUsd?: number;
  todos?: TodoSummary | null;
  taskPlan?: TaskPlanSummary | null;
  pid?: number | null;
  /** False when several agent processes share the cwd and the pid is a guess. */
  pidPrecise?: boolean;
  entrypoint?: string | null;
  userMark?: SessionMark | null;
  /** Unread = lastActivityMs > (lastReadMs ?? 0). */
  lastReadMs?: number | null;
  /** True when the session's agent process is still alive. */
  procAlive?: boolean;
}

/** Sessions Fleet spawned itself ("新会话" / handoff relay) — the only ones
 *  where SIGINT means "abort the tool call" instead of "quit". Mirrors
 *  claw-fleet-desktop/app/types.ts. */
export function isFleetOwnedEntrypoint(entrypoint: string | null | undefined): boolean {
  return entrypoint === "claw-fleet-newsession" || entrypoint === "claw-fleet-handoff";
}

export function isSessionUnread(s: SessionInfo): boolean {
  return s.lastActivityMs > (s.lastReadMs ?? 0);
}

const IN_FLIGHT: SessionStatus[] = [
  "thinking",
  "executing",
  "streaming",
  "processing",
  "active",
  "delegating",
];

/** Resumable = a Fleet-owned headless session whose process has ended and
 *  whose turn is not in flight (mirrors the desktop's canResumeSession). */
export function canResumeSession(s: SessionInfo): boolean {
  return (
    !s.isSubagent &&
    isFleetOwnedEntrypoint(s.entrypoint) &&
    !s.procAlive &&
    !IN_FLIGHT.includes(s.status)
  );
}

export interface TaskItem {
  text: string;
  done: boolean;
}

export interface TaskPlanDetail {
  id?: string | null;
  title?: string | null;
  source?: string | null;
  items: TaskItem[];
}

// ── Session detail (v2) ──────────────────────────────────────────────────────

/** One transcript jsonl record, loosely typed — we only look at a few fields. */
export interface ContentBlock {
  type: string;
  text?: string;
  thinking?: string;
  name?: string;
  input?: Record<string, unknown>;
  content?: unknown;
  /** tool_use block id / tool_result back-reference (preceding-narration slicing). */
  id?: string;
  tool_use_id?: string;
}

export interface RawMessage {
  type?: string;
  uuid?: string;
  timestamp?: string;
  isSidechain?: boolean;
  isCompactSummary?: boolean;
  message?: {
    role?: string;
    content?: string | ContentBlock[];
  };
}

/** `live_thinking` reply (null when no live sidecar). */
export interface LiveThinking {
  sessionId: string;
  thinking: string;
  streaming: boolean;
  updatedSecsAgo: number;
}

/** `handoff_chain` reply (null when the session is on no chain). */
export interface HandoffLink {
  fromSessionId: string;
  toSessionId: string;
  note: string;
  planId?: string | null;
  nextTask?: string | null;
  handedAt: number;
}

export interface HandoffChain {
  chainId: string;
  workspacePath: string;
  planId?: string | null;
  links: HandoffLink[];
}

/** `skill_history` reply items. */
export interface SkillInvocation {
  skill: string;
  args?: string | null;
  timestamp: string;
  isSubagent: boolean;
}

/** `workflow_trees` reply items (loosely typed — display only). */
export interface WorkflowAgentInfo {
  agentId?: string;
  label?: string | null;
  status?: string;
  prompt?: string | null;
  agentType?: string | null;
}

export interface WorkflowTree {
  runId: string;
  name?: string | null;
  description?: string | null;
  agents: WorkflowAgentInfo[];
}

/** `token_breakdown` reply — only the totals the mobile UI shows. */
export interface TokenBreakdown {
  totalsUsage?: {
    inputTokens?: number;
    outputTokens?: number;
    cacheCreationTokens?: number;
    cacheReadTokens?: number;
  };
  totalsEstimatedCostUsd?: number | null;
  main?: unknown;
  subagents?: unknown[];
}

/** `session_decisions` reply items — serde `#[serde(tag = "kind")]` envelope
 *  over the four record variants (claw-fleet-core/src/decision_history.rs). */
export interface SelectedOption {
  label: string;
  description?: string;
  other?: boolean;
}

interface DecisionRecordBase {
  id: string;
  sessionId: string;
  workspaceName?: string;
  aiTitle?: string | null;
  requestedAt?: string;
  resolvedAt?: string;
}

export interface ElicitationHistoryRecord extends DecisionRecordBase {
  kind: "elicitation";
  outcome: "answered" | "declined" | "heartbeat-lost" | "timeout";
  questions: ElicitationQuestion[];
  answers: Record<string, SelectedOption>;
}

export interface PlanApprovalHistoryRecord extends DecisionRecordBase {
  kind: "plan-approval";
  outcome: "approved" | "approved-with-edits" | "rejected" | "heartbeat-lost" | "timeout";
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

export interface FleetAskHistoryRecord extends DecisionRecordBase {
  kind: "fleet-ask";
  outcome: "answered" | "cancelled" | "heartbeat-lost" | "timeout";
  questions: FleetAskQuestion[];
  answers: Record<string, string>;
}

export type DecisionHistoryRecord =
  | ElicitationHistoryRecord
  | PlanApprovalHistoryRecord
  | UserPromptHistoryRecord
  | FleetAskHistoryRecord;
