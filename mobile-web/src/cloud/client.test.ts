import { describe, expect, it, vi } from "vitest";
import { HttpFleetCloudClient } from "./client";

const config = {
  baseUrl: "https://cloud.fleet.test/",
  organizationId: "org-1",
  projectId: "project-1",
  accessToken: "token-1",
  reconnectDelayMs: 0,
};

function jsonResponse(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { "content-type": "application/json" },
  });
}

describe("HttpFleetCloudClient", () => {
  it("lists scoped tasks with filters", async () => {
    const fetcher = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
      jsonResponse({ data: [], next_cursor: null, has_more: false }),
    );
    const client = new HttpFleetCloudClient(config, fetcher);

    await client.listTasks({ status: "running", cursor: "cursor-7", limit: 25 });

    const [url, init] = fetcher.mock.calls[0]!;
    expect(String(url)).toBe(
      "https://cloud.fleet.test/v1/tasks?status=running&cursor=cursor-7&limit=25",
    );
    expect(new Headers(init?.headers)).toMatchObject({});
    const headers = new Headers(init?.headers);
    expect(headers.get("authorization")).toBe("Bearer token-1");
    expect(headers.get("x-fleet-organization-id")).toBe("org-1");
    expect(headers.get("x-fleet-project-id")).toBe("project-1");
  });

  it("surfaces application/problem+json details", async () => {
    const fetcher = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
      jsonResponse({ code: "scope_denied", detail: "project is outside token scope" }, 403),
    );
    const client = new HttpFleetCloudClient(config, fetcher);

    await expect(client.getTask("task-1")).rejects.toThrow(
      "scope_denied: project is outside token scope",
    );
  });

  it("uses the Embed authorization scheme for scoped iframe clients", async () => {
    const fetcher = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
      jsonResponse({ id: "task-1", attempts: [], decisions: [] }),
    );
    const client = new HttpFleetCloudClient(
      { ...config, accessToken: undefined, embedToken: "embed-token-1" },
      fetcher,
    );

    await client.getTask("task-1");

    const headers = new Headers(fetcher.mock.calls[0]![1]?.headers);
    expect(headers.get("authorization")).toBe("Embed embed-token-1");
  });

  it("parses fragmented SSE frames and resumes after the supplied cursor", async () => {
    const encoder = new TextEncoder();
    const chunks = [
      "id: 8\nevent: task.status_changed\ndata: {\"id\":\"event-8\",\"task_id\":\"task-1\",",
      "\"sequence\":8,\"type\":\"task.status_changed\",\"data\":{\"status\":\"running\"}}\n\n",
      ": heartbeat\n\nid: 9\ndata: {\"id\":\"event-9\",\"task_id\":\"task-1\",\"sequence\":9,\"type\":\"transcript.message\",\"data\":{\"role\":\"assistant\",\"text\":\"done\"}}\n\n",
    ];
    const stream = new ReadableStream<Uint8Array>({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk));
        controller.close();
      },
    });
    const fetcher = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
      new Response(stream, { status: 200, headers: { "content-type": "text/event-stream" } }),
    );
    const client = new HttpFleetCloudClient(config, fetcher);
    const events: unknown[] = [];
    const controller = new AbortController();

    await client.streamTaskEvents("task-1", 7, (event) => {
      events.push(event);
      if (events.length === 2) controller.abort();
    }, controller.signal);

    expect(String(fetcher.mock.calls[0]![0])).toBe(
      "https://cloud.fleet.test/v1/tasks/task-1/events?after=7",
    );
    expect(events).toMatchObject([
      { sequence: 8, type: "task.status_changed", data: { status: "running" } },
      { sequence: 9, type: "transcript.message", data: { text: "done" } },
    ]);
  });

  it("reconnects from the last delivered event sequence", async () => {
    const encoder = new TextEncoder();
    const responseFor = (sequence: number) =>
      new Response(
        new ReadableStream<Uint8Array>({
          start(controller) {
            controller.enqueue(
              encoder.encode(
                `id: ${sequence}\ndata: {"id":"event-${sequence}","task_id":"task-1","sequence":${sequence},"type":"task.status_changed","data":{"status":"running"}}\n\n`,
              ),
            );
            controller.close();
          },
        }),
        { status: 200, headers: { "content-type": "text/event-stream" } },
      );
    let call = 0;
    const fetcher = vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) =>
      responseFor(++call === 1 ? 8 : 9),
    );
    const client = new HttpFleetCloudClient(config, fetcher);
    const controller = new AbortController();
    const sequences: number[] = [];

    await client.streamTaskEvents("task-1", 7, (event) => {
      sequences.push(event.sequence);
      if (sequences.length === 2) controller.abort();
    }, controller.signal);

    expect(sequences).toEqual([8, 9]);
    expect(fetcher.mock.calls.map(([url]) => String(url))).toEqual([
      "https://cloud.fleet.test/v1/tasks/task-1/events?after=7",
      "https://cloud.fleet.test/v1/tasks/task-1/events?after=8",
    ]);
  });
});
