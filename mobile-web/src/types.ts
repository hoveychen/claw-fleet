// Mirrors of the desktop app's TypeScript types (claw-fleet-desktop/app/types.ts)
// for the payloads that cross the relay. Field names are the serde camelCase
// forms — keep in sync with the Rust structs.

// ── Decision requests ────────────────────────────────────────────────────────

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

export type DecisionKind =
  | "guard"
  | "elicitation"
  | "fleet-ask"
  | "plan-approval"
  | "permission-prompt";

export type DecisionRequest =
  | GuardRequest
  | ElicitationRequest
  | FleetAskRequest
  | PlanApprovalRequest
  | PermissionPromptRequest;

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
