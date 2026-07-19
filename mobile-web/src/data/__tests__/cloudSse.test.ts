import { describe, expect, it, vi } from "vitest";
import { CloudEntityCache, openCloudEventStream } from "../cloudSse";
import type { Event } from "../FleetDataClient";

function event(id: string, type: string, data: Record<string, unknown>): Event {
  return {
    id: `evt_${id}`,
    cursor: id,
    organization_id: "org_1",
    project_id: "proj_1",
    type,
    occurred_at: "2026-01-01T00:00:00Z",
    recorded_at: "2026-01-01T00:00:00Z",
    data,
    schema_version: 1,
  };
}

function sse(...events: Event[]) {
  return new Response(
    events.map((value) => `id: ${value.cursor}\nevent: ${value.type}\ndata: ${JSON.stringify(value)}\n\n`).join(""),
    { status: 200, headers: { "content-type": "text/event-stream" } },
  );
}

async function flush() {
  await new Promise((resolve) => setTimeout(resolve, 0));
}

describe("cloud SSE", () => {
  it("persists cursor, reconnects with after, and deduplicates event ids", async () => {
    const first = event("10", "task.status_changed", { task_id: "task_1", status: "running" });
    const second = event("11", "task.status_changed", { task_id: "task_1", status: "succeeded" });
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(sse(first))
      .mockResolvedValueOnce(sse(first, second));
    const scheduled: Array<{ fn: () => void; delay: number }> = [];
    const received: Event[] = [];
    const subscription = openCloudEventStream({
      baseUrl: "https://cloud.example/api/v1",
      token: () => "memory-token",
      fetcher,
      input: { projectId: "proj_1", onEvent: (value) => received.push(value) },
      schedule: (fn, delay) => {
        scheduled.push({ fn, delay });
        return scheduled.length;
      },
      cancelSchedule: () => {},
    });
    await flush();
    expect(received.map((value) => value.cursor)).toEqual(["10"]);
    expect(subscription.cursor).toBe("10");
    expect(scheduled[0].delay).toBe(1_000);
    scheduled.shift()!.fn();
    await flush();
    expect(String(fetcher.mock.calls[1][0])).toContain("after=10");
    expect(received.map((value) => value.cursor)).toEqual(["10", "11"]);
    expect(new Headers(fetcher.mock.calls[0][1]?.headers).get("authorization")).toBe(
      "Bearer memory-token",
    );
    subscription.close();
  });

  it("uses capped exponential reconnect delays", async () => {
    const fetcher = vi.fn<typeof fetch>().mockRejectedValue(new Error("offline"));
    const scheduled: Array<{ fn: () => void; delay: number }> = [];
    const subscription = openCloudEventStream({
      baseUrl: "https://cloud.example",
      token: () => "token",
      fetcher,
      input: { onEvent: () => {} },
      schedule: (fn, delay) => {
        scheduled.push({ fn, delay });
        return scheduled.length;
      },
      cancelSchedule: () => {},
    });
    for (let index = 0; index < 7; index += 1) {
      await flush();
      scheduled[index]?.fn();
    }
    await flush();
    expect(scheduled.slice(0, 7).map(({ delay }) => delay)).toEqual([
      1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000,
    ]);
    subscription.close();
  });

  it("updates keyed Task and Decision cache", () => {
    const cache = new CloudEntityCache();
    cache.apply(event("1", "task.created", { task: { id: "task_1", status: "queued" } }));
    cache.apply(event("2", "task.status_changed", { task_id: "task_1", status: "running" }));
    cache.apply(event("3", "decision.created", { decision: { id: "dec_1", task_id: "task_1", status: "pending" } }));
    cache.apply(event("4", "decision.delivered", { decision_id: "dec_1", status: "answered" }));
    expect(cache.tasks.get("task_1")).toMatchObject({ id: "task_1", status: "running" });
    expect(cache.decisions.get("dec_1")).toMatchObject({ id: "dec_1", status: "answered" });
  });
});
