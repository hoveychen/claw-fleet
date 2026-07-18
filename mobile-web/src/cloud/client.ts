import type {
  CommandReceipt,
  CreateTaskInput,
  Decision,
  DecisionResponse,
  Page,
  Task,
  TaskDetail,
  TaskEvent,
  TaskStatus,
} from "./types";

export interface FleetCloudClient {
  listTasks(input: {
    status?: TaskStatus;
    cursor?: string;
    limit?: number;
  }): Promise<Page<Task>>;
  getTask(taskId: string): Promise<TaskDetail>;
  streamTaskEvents(
    taskId: string,
    after: number,
    onEvent: (event: TaskEvent) => void,
    signal: AbortSignal,
  ): Promise<void>;
  createTask(input: CreateTaskInput, idempotencyKey: string): Promise<Task>;
  sendMessage(taskId: string, text: string, idempotencyKey: string): Promise<CommandReceipt>;
  cancelTask(taskId: string, reason: string | null, idempotencyKey: string): Promise<CommandReceipt>;
  retryTask(taskId: string, idempotencyKey: string): Promise<CommandReceipt>;
  getDecision(decisionId: string): Promise<Decision>;
  respondToDecision(
    decisionId: string,
    response: DecisionResponse,
    idempotencyKey: string,
  ): Promise<Decision>;
}
