import type { Decision, Event, EventSubscription, StreamEventsInput, Task } from "./FleetDataClient";
import type { FetchLike } from "./CloudApiClient";

type Schedule = (fn: () => void, delay: number) => number;

export interface CloudSseOptions {
  baseUrl: string;
  token: () => string | null;
  fetcher: FetchLike;
  input: StreamEventsInput;
  schedule?: Schedule;
  cancelSchedule?: (id: number) => void;
}

class CloudEventSubscription implements EventSubscription {
  private readonly seen = new Set<string>();
  private readonly controller = new AbortController();
  private readonly schedule: Schedule;
  private readonly cancelSchedule: (id: number) => void;
  private timer: number | null = null;
  private closed = false;
  private reconnectAttempt = 0;
  private currentCursor: string | null;

  constructor(private readonly options: CloudSseOptions) {
    this.currentCursor = options.input.after ?? null;
    this.schedule =
      options.schedule ??
      ((fn, delay) => window.setTimeout(fn, delay));
    this.cancelSchedule = options.cancelSchedule ?? ((id) => window.clearTimeout(id));
    void this.connect();
  }

  get cursor(): string | null {
    return this.currentCursor;
  }

  close(): void {
    this.closed = true;
    this.controller.abort();
    if (this.timer !== null) this.cancelSchedule(this.timer);
    this.timer = null;
    this.options.input.onStatus?.(false);
  }

  private async connect(): Promise<void> {
    if (this.closed) return;
    const token = this.options.token();
    if (!token) {
      this.options.input.onError?.(new Error("Cloud authentication token is unavailable"));
      this.queueReconnect();
      return;
    }
    try {
      const url = new URL(`${this.options.baseUrl.replace(/\/$/, "")}/events/stream`);
      const input = this.options.input;
      if (input.projectId) url.searchParams.set("project_id", input.projectId);
      if (input.taskId) url.searchParams.set("task_id", input.taskId);
      if (input.types?.length) url.searchParams.set("types", input.types.join(","));
      if (this.currentCursor) url.searchParams.set("after", this.currentCursor);
      const response = await this.options.fetcher(url.toString(), {
        method: "GET",
        headers: {
          authorization: `Bearer ${token}`,
          accept: "text/event-stream",
        },
        signal: this.controller.signal,
      });
      if (!response.ok || !response.body) {
        throw new Error(`Cloud event stream failed with HTTP ${response.status}`);
      }
      this.reconnectAttempt = 0;
      input.onStatus?.(true);
      await consumeSse(response.body, (frame) => this.accept(frame));
      input.onStatus?.(false);
    } catch (error) {
      if (this.closed || this.controller.signal.aborted) return;
      this.options.input.onStatus?.(false);
      this.options.input.onError?.(error);
    }
    this.queueReconnect();
  }

  private accept(frame: SseFrame): void {
    if (!frame.data) return;
    let event: Event;
    try {
      event = JSON.parse(frame.data) as Event;
    } catch (error) {
      this.options.input.onError?.(error);
      return;
    }
    const id = frame.id || event.cursor || event.id;
    if (this.seen.has(id)) return;
    this.seen.add(id);
    if (this.seen.size > 2_048) {
      const oldest = this.seen.values().next().value as string | undefined;
      if (oldest) this.seen.delete(oldest);
    }
    this.currentCursor = frame.id || event.cursor;
    this.options.input.onEvent(event);
  }

  private queueReconnect(): void {
    if (this.closed) return;
    const delay = Math.min(30_000, 1_000 * 2 ** this.reconnectAttempt);
    this.reconnectAttempt += 1;
    this.timer = this.schedule(() => {
      this.timer = null;
      void this.connect();
    }, delay);
  }
}

interface SseFrame {
  id: string;
  event: string;
  data: string;
}

async function consumeSse(
  stream: ReadableStream<Uint8Array>,
  onFrame: (frame: SseFrame) => void,
): Promise<void> {
  const reader = stream.getReader();
  const decoder = new TextDecoder();
  let buffer = "";
  for (;;) {
    const { value, done } = await reader.read();
    buffer += decoder.decode(value, { stream: !done }).replace(/\r\n/g, "\n");
    let boundary = buffer.indexOf("\n\n");
    while (boundary >= 0) {
      const block = buffer.slice(0, boundary);
      buffer = buffer.slice(boundary + 2);
      const frame = parseFrame(block);
      if (frame) onFrame(frame);
      boundary = buffer.indexOf("\n\n");
    }
    if (done) break;
  }
  const tail = parseFrame(buffer);
  if (tail) onFrame(tail);
}

function parseFrame(block: string): SseFrame | null {
  const frame: SseFrame = { id: "", event: "message", data: "" };
  const data: string[] = [];
  for (const line of block.split("\n")) {
    if (!line || line.startsWith(":")) continue;
    const separator = line.indexOf(":");
    const field = separator < 0 ? line : line.slice(0, separator);
    const value = separator < 0 ? "" : line.slice(separator + 1).replace(/^ /, "");
    if (field === "id") frame.id = value;
    else if (field === "event") frame.event = value;
    else if (field === "data") data.push(value);
  }
  frame.data = data.join("\n");
  return frame.data || frame.id ? frame : null;
}

export function openCloudEventStream(options: CloudSseOptions): EventSubscription {
  return new CloudEventSubscription(options);
}

export class CloudEntityCache {
  readonly tasks = new Map<string, Partial<Task>>();
  readonly decisions = new Map<string, Partial<Decision>>();

  apply(event: Event): void {
    const data = event.data as Record<string, unknown>;
    if (event.type.startsWith("task.")) {
      const task = objectValue(data.task);
      const id = stringValue(task?.id) ?? stringValue(data.task_id) ?? event.task_id ?? null;
      if (id) this.tasks.set(id, { ...this.tasks.get(id), ...(task ?? data), id });
    }
    if (event.type.startsWith("decision.")) {
      const decision = objectValue(data.decision);
      const id =
        stringValue(decision?.id) ?? stringValue(data.decision_id) ?? null;
      if (id) this.decisions.set(id, { ...this.decisions.get(id), ...(decision ?? data), id });
    }
  }
}

function objectValue(value: unknown): Record<string, unknown> | null {
  return value !== null && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null;
}

function stringValue(value: unknown): string | null {
  return typeof value === "string" ? value : null;
}
