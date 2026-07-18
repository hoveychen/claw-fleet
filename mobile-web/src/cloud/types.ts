export type TaskStatus =
  | "queued"
  | "assigned"
  | "running"
  | "waiting_for_input"
  | "paused"
  | "rate_limited"
  | "succeeded"
  | "failed"
  | "cancelled";

export interface Task {
  id: string;
  organization_id: string;
  project_id: string;
  external_id: string | null;
  title: string | null;
  prompt: string;
  status: TaskStatus;
  current_attempt_id: string | null;
  waiting_decision_count: number;
  event_cursor: number;
  version: number;
  created_at: string;
  updated_at: string;
}

export interface Attempt {
  id: string;
  task_id: string;
  runner_id: string;
  workspace_id: string;
  agent_source: "claude" | "codex";
  agent_session_id: string | null;
  ordinal: number;
  reason: "initial" | "handoff" | "retry" | "resume" | "recovery";
  status: "starting" | "running" | "waiting" | "ended" | "lost";
  started_at: string;
  ended_at: string | null;
}

export type DecisionKind =
  | "guard"
  | "elicitation"
  | "fleet_ask"
  | "plan_approval"
  | "permission_prompt"
  | "a2ui";

export interface Decision {
  id: string;
  task_id: string;
  attempt_id: string;
  kind: DecisionKind;
  blocking: boolean;
  schema_version: string;
  presentation: Record<string, unknown>;
  status: "open" | "answered" | "declined" | "expired" | "cancelled";
  response: Record<string, unknown> | null;
  created_at: string;
  resolved_at: string | null;
}

export interface TaskEvent<T extends Record<string, unknown> = Record<string, unknown>> {
  id: string;
  organization_id: string;
  project_id: string;
  task_id: string;
  attempt_id: string | null;
  sequence: number;
  type: string;
  occurred_at: string;
  recorded_at: string;
  producer: {
    type: "control_plane" | "runner" | "user" | "api_client";
    id: string;
  };
  schema_version: string;
  data: T;
}

export interface CommandReceipt {
  command_id: string;
  status: "accepted" | "already_applied";
  accepted_at: string;
}

export interface Page<T> {
  data: T[];
  next_cursor: string | null;
  has_more: boolean;
}

export interface CreateTaskInput {
  external_id?: string | null;
  title?: string | null;
  prompt: string;
  workspace_selector: {
    workspace_id?: string | null;
    labels?: Record<string, string>;
  };
  agent_profile: {
    tool: "claude" | "codex";
    model?: string | null;
    effort?: string | null;
    permission_policy?: string | null;
    required_capabilities?: string[];
  };
  metadata?: Record<string, string>;
}

export interface DecisionResponse {
  action: "answer" | "decline";
  answers?: Record<string, unknown>;
}

export interface TaskDetail extends Task {
  attempts: Attempt[];
  decisions: Decision[];
}
