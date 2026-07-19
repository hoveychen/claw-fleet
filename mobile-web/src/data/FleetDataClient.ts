import type { components } from "../generated/cloud-api";

export type Task = components["schemas"]["Task"];
export type Decision = components["schemas"]["Decision"];
export type Event = components["schemas"]["Event"];
export type TranscriptRecord = components["schemas"]["TranscriptRecord"];
export type Run = components["schemas"]["Run"];
export type Usage = components["schemas"]["Usage"];
export type CommandReceipt = components["schemas"]["CommandReceipt"];
export type CreateTaskInput = components["schemas"]["CreateTaskRequest"];
export type CreateTaskResponse = components["schemas"]["CreateTaskResponse"];
export type AppendMessageInput = components["schemas"]["AppendMessageRequest"];
export type DecisionResponseInput = components["schemas"]["DecisionResponseRequest"];

export interface Page<T> {
  data: T[];
  next_cursor?: string | null;
}

export interface ListTasksInput {
  projectId?: string;
  status?: components["schemas"]["TaskStatus"];
  externalId?: string;
  cursor?: string;
  limit?: number;
}

export interface ListDecisionsInput {
  projectId?: string;
  taskId?: string;
  status?: components["schemas"]["DecisionStatus"];
  cursor?: string;
  limit?: number;
}

export interface DecisionResponse {
  decision: Decision;
  command: CommandReceipt;
}

export interface StreamEventsInput {
  projectId?: string;
  taskId?: string;
  types?: string[];
  after?: string;
  onEvent: (event: Event) => void;
  onStatus?: (connected: boolean) => void;
  onError?: (error: unknown) => void;
}

export interface EventSubscription {
  close(): void;
  readonly cursor: string | null;
}

export interface FleetDataClient {
  listTasks(input: ListTasksInput): Promise<Page<Task>>;
  getTask(taskId: string): Promise<Task>;
  createTask(input: CreateTaskInput, idempotencyKey: string): Promise<CreateTaskResponse>;
  appendMessage(
    taskId: string,
    input: AppendMessageInput,
    idempotencyKey: string,
  ): Promise<CommandReceipt>;
  controlTask(
    taskId: string,
    action: "cancel" | "pause" | "resume",
    version: number,
    idempotencyKey: string,
  ): Promise<CommandReceipt>;
  listDecisions(input: ListDecisionsInput): Promise<Page<Decision>>;
  respondToDecision(
    decisionId: string,
    version: number,
    input: DecisionResponseInput,
    idempotencyKey: string,
  ): Promise<DecisionResponse>;
  listRunMessages(runId: string, after?: string): Promise<Page<TranscriptRecord>>;
  getRun(runId: string): Promise<Run>;
  getRunUsage(runId: string): Promise<Usage>;
  streamEvents(input: StreamEventsInput): EventSubscription;
}
