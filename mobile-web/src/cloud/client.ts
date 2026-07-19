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

export interface FleetCloudClientConfig {
  baseUrl: string;
  organizationId: string;
  projectId: string;
  accessToken?: string;
  embedToken?: string;
  reconnectDelayMs?: number;
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;

interface ProblemDetails {
  code?: string;
  detail?: string;
  title?: string;
}

export class FleetCloudError extends Error {
  constructor(
    message: string,
    readonly status: number,
    readonly code: string,
  ) {
    super(message);
    this.name = "FleetCloudError";
  }
}

export class HttpFleetCloudClient implements FleetCloudClient {
  private readonly baseUrl: string;

  constructor(
    private readonly config: FleetCloudClientConfig,
    private readonly fetcher: Fetcher = globalThis.fetch.bind(globalThis),
  ) {
    this.baseUrl = config.baseUrl.replace(/\/+$/, "");
  }

  listTasks(input: {
    status?: TaskStatus;
    cursor?: string;
    limit?: number;
  }): Promise<Page<Task>> {
    const query = new URLSearchParams();
    if (input.status) query.set("status", input.status);
    if (input.cursor) query.set("cursor", input.cursor);
    if (input.limit != null) query.set("limit", String(input.limit));
    const suffix = query.size > 0 ? `?${query}` : "";
    return this.request(`/v1/tasks${suffix}`);
  }

  getTask(taskId: string): Promise<TaskDetail> {
    return this.request(`/v1/tasks/${encodeURIComponent(taskId)}`);
  }

  async streamTaskEvents(
    taskId: string,
    after: number,
    onEvent: (event: TaskEvent) => void,
    signal: AbortSignal,
  ): Promise<void> {
    let cursor = Math.max(0, after);
    while (!signal.aborted) {
      try {
        const response = await this.fetcher(
          this.url(`/v1/tasks/${encodeURIComponent(taskId)}/events?after=${cursor}`),
          { headers: this.headers(), signal },
        );
        await this.ensureOk(response);
        if (!response.body) {
          throw new FleetCloudError("event stream has no response body", 502, "empty_stream");
        }

        const reader = response.body.getReader();
        const decoder = new TextDecoder();
        let buffer = "";
        while (!signal.aborted) {
          const { done, value } = await reader.read();
          buffer += decoder.decode(value, { stream: !done }).replace(/\r\n/g, "\n");
          let boundary = buffer.indexOf("\n\n");
          while (boundary >= 0) {
            const frame = buffer.slice(0, boundary);
            buffer = buffer.slice(boundary + 2);
            const data = frame
              .split("\n")
              .filter((line) => line.startsWith("data:"))
              .map((line) => line.slice(5).trimStart())
              .join("\n");
            if (data) {
              const event = JSON.parse(data) as TaskEvent;
              if (event.sequence > cursor) {
                cursor = event.sequence;
                onEvent(event);
              }
            }
            boundary = buffer.indexOf("\n\n");
          }
          if (done) break;
        }
      } catch (error) {
        if (signal.aborted) return;
        if (error instanceof FleetCloudError && error.status >= 400 && error.status < 500) {
          throw error;
        }
      }
      if (!signal.aborted) {
        await waitForReconnect(this.config.reconnectDelayMs ?? 750, signal);
      }
    }
  }

  createTask(input: CreateTaskInput, idempotencyKey: string): Promise<Task> {
    return this.request("/v1/tasks", { method: "POST", body: JSON.stringify(input) }, idempotencyKey);
  }

  sendMessage(taskId: string, text: string, idempotencyKey: string): Promise<CommandReceipt> {
    return this.request(
      `/v1/tasks/${encodeURIComponent(taskId)}/messages`,
      { method: "POST", body: JSON.stringify({ text }) },
      idempotencyKey,
    );
  }

  cancelTask(taskId: string, reason: string | null, idempotencyKey: string): Promise<CommandReceipt> {
    return this.request(
      `/v1/tasks/${encodeURIComponent(taskId)}/cancel`,
      { method: "POST", body: JSON.stringify({ reason }) },
      idempotencyKey,
    );
  }

  retryTask(taskId: string, idempotencyKey: string): Promise<CommandReceipt> {
    return this.request(
      `/v1/tasks/${encodeURIComponent(taskId)}/retry`,
      { method: "POST", body: "{}" },
      idempotencyKey,
    );
  }

  getDecision(decisionId: string): Promise<Decision> {
    return this.request(`/v1/decisions/${encodeURIComponent(decisionId)}`);
  }

  respondToDecision(
    decisionId: string,
    response: DecisionResponse,
    idempotencyKey: string,
  ): Promise<Decision> {
    return this.request(
      `/v1/decisions/${encodeURIComponent(decisionId)}/responses`,
      { method: "POST", body: JSON.stringify(response) },
      idempotencyKey,
    );
  }

  private url(path: string): string {
    return `${this.baseUrl}${path}`;
  }

  private headers(idempotencyKey?: string): Headers {
    const headers = new Headers({
      accept: "application/json",
      "x-fleet-organization-id": this.config.organizationId,
      "x-fleet-project-id": this.config.projectId,
    });
    if (this.config.embedToken) {
      headers.set("authorization", `Embed ${this.config.embedToken}`);
    } else if (this.config.accessToken) {
      headers.set("authorization", `Bearer ${this.config.accessToken}`);
    }
    if (idempotencyKey) headers.set("idempotency-key", idempotencyKey);
    return headers;
  }

  private async request<T>(path: string, init: RequestInit = {}, idempotencyKey?: string): Promise<T> {
    const headers = this.headers(idempotencyKey);
    if (init.body != null) headers.set("content-type", "application/json");
    const response = await this.fetcher(this.url(path), { ...init, headers });
    await this.ensureOk(response);
    return (await response.json()) as T;
  }

  private async ensureOk(response: Response): Promise<void> {
    if (response.ok) return;
    let problem: ProblemDetails = {};
    try {
      problem = (await response.json()) as ProblemDetails;
    } catch {
      // Non-JSON proxy failures still carry a useful HTTP status below.
    }
    const code = problem.code ?? `http_${response.status}`;
    const detail = (problem.detail ?? problem.title ?? response.statusText) || "request failed";
    throw new FleetCloudError(`${code}: ${detail}`, response.status, code);
  }
}

function waitForReconnect(delayMs: number, signal: AbortSignal): Promise<void> {
  if (delayMs <= 0 || signal.aborted) return Promise.resolve();
  return new Promise((resolve) => {
    const timer = globalThis.setTimeout(resolve, delayMs);
    signal.addEventListener(
      "abort",
      () => {
        globalThis.clearTimeout(timer);
        resolve();
      },
      { once: true },
    );
  });
}
