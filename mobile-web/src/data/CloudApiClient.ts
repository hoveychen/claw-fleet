import type {
  AppendMessageInput,
  CommandReceipt,
  CreateTaskInput,
  CreateTaskResponse,
  DecisionResponse,
  DecisionResponseInput,
  EventSubscription,
  FleetDataClient,
  ListDecisionsInput,
  ListTasksInput,
  Page,
  StreamEventsInput,
  Task,
  Decision,
  TranscriptRecord,
  Run,
  Usage,
  OperationsSnapshot,
} from "./FleetDataClient";
import { openCloudEventStream } from "./cloudSse";

export type FetchLike = typeof fetch;

export interface CloudApiClientOptions {
  baseUrl: string;
  token: string | (() => string | null);
  fetcher?: FetchLike;
  headers?: Record<string, string>;
}

interface ErrorEnvelope {
  error?: {
    type?: string;
    code?: string;
    message?: string;
    request_id?: string;
  };
}

export class CloudApiError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
    readonly requestId: string | null,
    readonly errorType: string | null,
  ) {
    super(message);
    this.name = "CloudApiError";
  }
}

export class CloudApiClient implements FleetDataClient {
  readonly baseUrl: string;
  private readonly tokenSource: () => string | null;
  readonly fetcher: FetchLike;
  private readonly fixedHeaders: Record<string, string>;

  constructor(options: CloudApiClientOptions) {
    this.baseUrl = options.baseUrl.replace(/\/$/, "");
    this.tokenSource =
      typeof options.token === "function" ? options.token : () => options.token as string;
    this.fetcher = options.fetcher ?? fetch.bind(globalThis);
    this.fixedHeaders = options.headers ?? {};
  }

  token(): string | null {
    return this.tokenSource();
  }

  listTasks(input: ListTasksInput): Promise<Page<Task>> {
    return this.request("GET", `/tasks${query({
      project_id: input.projectId,
      status: input.status,
      external_id: input.externalId,
      cursor: input.cursor,
      limit: input.limit,
    })}`);
  }

  getTask(taskId: string): Promise<Task> {
    return this.request("GET", `/tasks/${encodeURIComponent(taskId)}`);
  }

  createTask(input: CreateTaskInput, idempotencyKey: string): Promise<CreateTaskResponse> {
    return this.request("POST", "/tasks", input, { "idempotency-key": idempotencyKey });
  }

  async appendMessage(
    taskId: string,
    input: AppendMessageInput,
    idempotencyKey: string,
  ): Promise<CommandReceipt> {
    const body = await this.request<CommandReceipt | { command: CommandReceipt }>(
      "POST",
      `/tasks/${encodeURIComponent(taskId)}/messages`,
      input,
      { "idempotency-key": idempotencyKey },
    );
    return commandFrom(body);
  }

  async controlTask(
    taskId: string,
    action: "cancel" | "pause" | "resume",
    version: number,
    idempotencyKey: string,
  ): Promise<CommandReceipt> {
    const body = await this.request<CommandReceipt | { command: CommandReceipt }>(
      "POST",
      `/tasks/${encodeURIComponent(taskId)}/${action}`,
      {},
      { "idempotency-key": idempotencyKey, "if-match": `"${version}"` },
    );
    return commandFrom(body);
  }

  listDecisions(input: ListDecisionsInput): Promise<Page<Decision>> {
    return this.request("GET", `/decisions${query({
      project_id: input.projectId,
      task_id: input.taskId,
      status: input.status,
      cursor: input.cursor,
      limit: input.limit,
    })}`);
  }

  respondToDecision(
    decisionId: string,
    version: number,
    input: DecisionResponseInput,
    idempotencyKey: string,
  ): Promise<DecisionResponse> {
    return this.request(
      "POST",
      `/decisions/${encodeURIComponent(decisionId)}/responses`,
      input,
      {
        "idempotency-key": idempotencyKey,
        "if-match": `"decision-version-${version}"`,
      },
    );
  }

  listRunMessages(runId: string, after?: string): Promise<Page<TranscriptRecord>> {
    return this.request(
      "GET",
      `/runs/${encodeURIComponent(runId)}/messages${query({ after })}`,
    );
  }

  getRun(runId: string): Promise<Run> {
    return this.request("GET", `/runs/${encodeURIComponent(runId)}`);
  }

  getRunUsage(runId: string): Promise<Usage> {
    return this.request("GET", `/runs/${encodeURIComponent(runId)}/usage`);
  }

  getOperations(): Promise<OperationsSnapshot> {
    return this.request("GET", "/operations");
  }

  streamEvents(input: StreamEventsInput): EventSubscription {
    return openCloudEventStream({
      baseUrl: this.baseUrl,
      token: () => this.token(),
      fetcher: this.fetcher,
      headers: this.fixedHeaders,
      input,
    });
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    extraHeaders: Record<string, string> = {},
  ): Promise<T> {
    const token = this.token();
    if (!token) {
      throw new CloudApiError("Cloud authentication token is unavailable", 401, "token_missing", null, "authentication");
    }
    const headers = new Headers(this.fixedHeaders);
    for (const [name, value] of Object.entries(extraHeaders)) headers.set(name, value);
    headers.set("authorization", `Bearer ${token}`);
    headers.set("accept", "application/json");
    let payload: BodyInit | undefined;
    if (body !== undefined) {
      headers.set("content-type", "application/json");
      payload = JSON.stringify(body);
    }
    const response = await this.fetcher(`${this.baseUrl}${path}`, {
      method,
      headers,
      body: payload,
    });
    const parsed = await parseJson(response);
    if (!response.ok) {
      const envelope = parsed as ErrorEnvelope | null;
      const error = envelope?.error;
      throw new CloudApiError(
        error?.message ?? `Cloud request failed with HTTP ${response.status}`,
        response.status,
        error?.code ?? "http_error",
        response.headers.get("fleet-request-id") ?? error?.request_id ?? null,
        error?.type ?? null,
      );
    }
    return parsed as T;
  }
}

function query(values: Record<string, string | number | undefined>): string {
  const params = new URLSearchParams();
  for (const [key, value] of Object.entries(values)) {
    if (value !== undefined) params.set(key, String(value));
  }
  const encoded = params.toString();
  return encoded ? `?${encoded}` : "";
}

async function parseJson(response: Response): Promise<unknown> {
  const text = await response.text();
  if (!text) return null;
  try {
    return JSON.parse(text);
  } catch {
    throw new CloudApiError(
      "Cloud returned an invalid JSON response",
      response.status,
      "invalid_response",
      response.headers.get("fleet-request-id"),
      "protocol",
    );
  }
}

function commandFrom(body: CommandReceipt | { command: CommandReceipt }): CommandReceipt {
  return "command" in body ? body.command : body;
}
