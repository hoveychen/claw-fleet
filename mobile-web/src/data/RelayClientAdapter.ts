import type { RelayClient } from "../relay";
import type { PendingDecision, SessionInfo } from "../types";
import type {
  AppendMessageInput,
  CommandReceipt,
  CreateTaskInput,
  CreateTaskResponse,
  Decision,
  DecisionResponse,
  DecisionResponseInput,
  EventSubscription,
  FleetDataClient,
  ListDecisionsInput,
  ListTasksInput,
  Page,
  StreamEventsInput,
  Task,
  TranscriptRecord,
  Run,
  Usage,
} from "./FleetDataClient";

interface RelaySnapshot {
  sessions: SessionInfo[];
  decisions: PendingDecision[];
}

export class RelayClientAdapter implements FleetDataClient {
  constructor(
    private readonly relay: Pick<RelayClient, "request" | "answer">,
    private readonly snapshot: () => RelaySnapshot,
  ) {}

  async listTasks(input: ListTasksInput): Promise<Page<Task>> {
    let tasks = this.snapshot().sessions.map(sessionToTask);
    if (input.status) tasks = tasks.filter((task) => task.status === input.status);
    const start = input.cursor ? Number(input.cursor) || 0 : 0;
    const limit = input.limit ?? 50;
    return {
      data: tasks.slice(start, start + limit),
      next_cursor: start + limit < tasks.length ? String(start + limit) : null,
    };
  }

  async getTask(taskId: string): Promise<Task> {
    const session = this.snapshot().sessions.find((item) => item.id === taskId);
    if (!session) throw new Error(`Relay Task ${taskId} is unavailable`);
    return sessionToTask(session);
  }

  async createTask(input: CreateTaskInput, _idempotencyKey: string): Promise<CreateTaskResponse> {
    const sessionId = crypto.randomUUID();
    await this.relay.request("spawn_session", {
      workspacePath: input.workspace.repository,
      prompt: input.goal,
      sessionId,
      tool: input.agent.provider === "codex" ? "codex" : "claude",
      model: input.agent.model,
      effort: input.agent.effort,
    });
    const now = new Date().toISOString();
    const task = taskFromCreate(sessionId, input, now);
    return {
      task,
      run: {
        id: sessionId,
        task_id: sessionId,
        attempt: 1,
        status: "starting",
        agent: input.agent,
        version: 1,
        created_at: now,
        updated_at: now,
      },
    };
  }

  async appendMessage(
    taskId: string,
    input: AppendMessageInput,
    _idempotencyKey: string,
  ): Promise<CommandReceipt> {
    const session = this.snapshot().sessions.find((item) => item.id === taskId);
    if (!session) throw new Error(`Relay Task ${taskId} is unavailable`);
    await this.relay.request(session.procAlive ? "enqueue_message" : "resume_session", {
      sessionId: session.id,
      workspacePath: session.workspacePath,
      text: input.text,
      prompt: input.text,
      agentSource: session.agentSource ?? "",
    });
    return receipt();
  }

  async controlTask(
    taskId: string,
    action: "cancel" | "pause" | "resume",
    _version: number,
    _idempotencyKey: string,
  ): Promise<CommandReceipt> {
    const session = this.snapshot().sessions.find((item) => item.id === taskId);
    if (!session) throw new Error(`Relay Task ${taskId} is unavailable`);
    if (action === "pause") throw new Error("Pause is available only in Fleet Cloud mode");
    if (action === "cancel") {
      await this.relay.request("interrupt", { sessionId: session.id, pid: session.pid });
    } else {
      await this.relay.request("resume_session", {
        sessionId: session.id,
        workspacePath: session.workspacePath,
        agentSource: session.agentSource ?? "",
      });
    }
    return receipt();
  }

  async listDecisions(input: ListDecisionsInput): Promise<Page<Decision>> {
    let decisions = this.snapshot().decisions.map(pendingToDecision);
    if (input.taskId) decisions = decisions.filter((decision) => decision.task_id === input.taskId);
    if (input.status) decisions = decisions.filter((decision) => decision.status === input.status);
    return { data: decisions, next_cursor: null };
  }

  async respondToDecision(
    decisionId: string,
    _version: number,
    input: DecisionResponseInput,
    _idempotencyKey: string,
  ): Promise<DecisionResponse> {
    const pending = this.snapshot().decisions.find((item) => item.id === decisionId);
    if (!pending) throw new Error(`Relay Decision ${decisionId} is unavailable`);
    const sent = this.relay.answer(pending.kind, decisionId, {
      action: input.action,
      answers: input.answers ?? {},
    });
    if (!sent) throw new Error("Relay is disconnected");
    const decision = pendingToDecision(pending);
    decision.status = input.action === "cancel" ? "cancelled" : input.action === "decline" ? "declined" : "answered";
    return { decision, command: receipt() };
  }

  async listRunMessages(runId: string, _after?: string): Promise<Page<TranscriptRecord>> {
    const session = this.snapshot().sessions.find((item) => item.id === runId);
    if (!session) throw new Error(`Relay Run ${runId} is unavailable`);
    const rows = await this.relay.request<Array<Record<string, unknown>>>("tail", {
      path: session.jsonlPath,
      n: 500,
    });
    return {
      data: rows.map((record, index) => ({
        id: `${runId}:${index + 1}`,
        run_id: runId,
        sequence: index + 1,
        role: relayRole(record),
        content: relayContent(record),
        occurred_at:
          typeof record.timestamp === "string" ? record.timestamp : new Date().toISOString(),
      })),
      next_cursor: null,
    };
  }

  async getRun(runId: string): Promise<Run> {
    const session = this.snapshot().sessions.find((item) => item.id === runId);
    if (!session) throw new Error(`Relay Run ${runId} is unavailable`);
    const updated = new Date(session.lastActivityMs).toISOString();
    return {
      id: runId,
      task_id: session.id,
      attempt: 1,
      status: session.procAlive ? "running" : "succeeded",
      agent: { provider: session.agentSource === "codex" ? "codex" : "claude_code" },
      version: 1,
      created_at: updated,
      updated_at: updated,
    };
  }

  async getRunUsage(_runId: string): Promise<Usage> {
    return {
      input_tokens: 0, output_tokens: 0, cache_read_tokens: 0, cache_write_tokens: 0,
      reasoning_tokens: 0, provider_cost_usd: 0, run_seconds: 0,
      event_bytes: 0, artifact_bytes: 0,
    };
  }

  streamEvents(_input: StreamEventsInput): EventSubscription {
    return { cursor: null, close() {} };
  }
}

function sessionToTask(session: SessionInfo): Task {
  const updated = new Date(session.lastActivityMs).toISOString();
  return {
    id: session.id,
    project_id: session.workspacePath,
    title: session.aiTitle ?? session.slug ?? session.workspaceName,
    goal: session.lastMessagePreview ?? session.aiTitle ?? "Fleet session",
    status: sessionStatus(session),
    active_run_id: session.id,
    version: 1,
    created_at: updated,
    updated_at: updated,
    metadata: { relay_mode: true },
  };
}

function sessionStatus(session: SessionInfo): Task["status"] {
  if (session.procAlive) return "running";
  if (session.status === "serverErrored") return "failed";
  return "succeeded";
}

function taskFromCreate(id: string, input: CreateTaskInput, now: string): Task {
  return {
    id,
    project_id: input.project_id,
    external_id: input.external_id,
    title: input.title,
    goal: input.goal,
    status: "queued",
    active_run_id: id,
    metadata: input.metadata,
    version: 1,
    created_at: now,
    updated_at: now,
  };
}

function pendingToDecision(pending: PendingDecision): Decision {
  const request = pending.request as unknown as Record<string, unknown>;
  const sessionId = typeof request.sessionId === "string" ? request.sessionId : "relay";
  return {
    id: pending.id,
    task_id: sessionId,
    run_id: sessionId,
    kind: pending.kind === "fleet-ask" ? "fleet_ask" : pending.kind.replaceAll("-", "_") as Decision["kind"],
    status: "pending",
    schema_version: 1,
    payload: request,
    response_schema: {},
    version: 1,
    created_at: new Date(pending.arrivedAt).toISOString(),
    updated_at: new Date(pending.arrivedAt).toISOString(),
  };
}

function receipt(): CommandReceipt {
  return {
    command_id: crypto.randomUUID(),
    status: "accepted",
    accepted_at: new Date().toISOString(),
  };
}

function relayRole(record: Record<string, unknown>): "system" | "user" | "assistant" | "tool" {
  const message = record.message as Record<string, unknown> | undefined;
  const role = message?.role;
  return role === "user" || role === "assistant" || role === "system" ? role : "tool";
}

function relayContent(record: Record<string, unknown>): Array<{ type: string; [key: string]: unknown }> {
  const message = record.message as Record<string, unknown> | undefined;
  const content = message?.content;
  if (Array.isArray(content)) return content as Array<{ type: string; [key: string]: unknown }>;
  return [{ type: "text", text: typeof content === "string" ? content : JSON.stringify(record) }];
}
