import { describe, expect, it, vi } from "vitest";
import { CloudApiClient, CloudApiError } from "../CloudApiClient";

function json(body: unknown, init: ResponseInit = {}) {
  return new Response(JSON.stringify(body), {
    status: 200,
    headers: { "content-type": "application/json", ...(init.headers ?? {}) },
    ...init,
  });
}

describe("CloudApiClient", () => {
  it("binds the browser fetch implementation before storing it as an instance method", async () => {
    const original = globalThis.fetch;
    const receiver = vi.fn(function (this: unknown) {
      if (this !== globalThis) throw new TypeError("Illegal invocation");
      return Promise.resolve(json({ data: [], next_cursor: null }));
    }) as typeof fetch;
    globalThis.fetch = receiver;
    try {
      const client = new CloudApiClient({ baseUrl: "https://cloud.example", token: "token" });
      await client.listTasks({});
      expect(receiver).toHaveBeenCalledOnce();
    } finally {
      globalThis.fetch = original;
    }
  });

  it("injects bearer auth and mutation idempotency headers", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockResolvedValueOnce(json({ task: { id: "task_1" }, run: { id: "run_1" } }, { status: 202 }))
      .mockResolvedValueOnce(json({ command_id: "cmd_1", status: "accepted", accepted_at: "2026-01-01T00:00:00Z" }, { status: 202 }))
      .mockResolvedValueOnce(json({ command_id: "cmd_2", status: "accepted", accepted_at: "2026-01-01T00:00:00Z" }, { status: 202 }))
      .mockResolvedValueOnce(json({ decision: { id: "dec_1" }, command: { command_id: "cmd_3", status: "accepted", accepted_at: "2026-01-01T00:00:00Z" } }, { status: 202 }));
    const client = new CloudApiClient({ baseUrl: "https://cloud.example/api/v1", token: () => "memory-token", fetcher });

    await client.createTask({ project_id: "proj_1", goal: "ship", workspace: { repository: "github:org/repo", ref: "main" }, agent: { provider: "codex" } }, "idem-create");
    await client.appendMessage("task_1", { text: "continue" }, "idem-message");
    await client.controlTask("task_1", "pause", 7, "idem-control");
    await client.respondToDecision("dec_1", 9, { action: "answer", answers: {} }, "idem-decision");

    const calls = fetcher.mock.calls;
    expect(new Headers(calls[0][1]?.headers).get("authorization")).toBe("Bearer memory-token");
    expect(new Headers(calls[0][1]?.headers).get("idempotency-key")).toBe("idem-create");
    expect(new Headers(calls[1][1]?.headers).get("idempotency-key")).toBe("idem-message");
    expect(new Headers(calls[2][1]?.headers).get("if-match")).toBe('"7"');
    expect(new Headers(calls[3][1]?.headers).get("if-match")).toBe('"decision-version-9"');
    expect(new Headers(calls[3][1]?.headers).get("idempotency-key")).toBe("idem-decision");
  });

  it("normalizes ErrorEnvelope and captures request id", async () => {
    const fetcher = vi.fn<typeof fetch>().mockResolvedValue(
      json(
        { error: { type: "conflict", code: "version_conflict", message: "stale", request_id: "req_body" } },
        { status: 412, headers: { "fleet-request-id": "req_header" } },
      ),
    );
    const client = new CloudApiClient({ baseUrl: "https://cloud.example", token: "token", fetcher });
    const error = await client.getTask("task_1").catch((value) => value);
    expect(error).toBeInstanceOf(CloudApiError);
    expect(error).toMatchObject({ status: 412, code: "version_conflict", requestId: "req_header", message: "stale" });
  });

  it("builds list and transcript cursors without leaking token into the URL", async () => {
    const fetcher = vi
      .fn<typeof fetch>()
      .mockImplementation(async () => json({ data: [], next_cursor: null }));
    const client = new CloudApiClient({ baseUrl: "https://cloud.example/api/v1/", token: "super-secret", fetcher });
    await client.listTasks({ projectId: "proj_1", status: "running", cursor: "cur_1" });
    await client.listRunMessages("run_1", "41");
    await client.getRun("run_1");
    await client.getRunUsage("run_1");
    await client.getOperations();
    expect(String(fetcher.mock.calls[0][0])).toBe("https://cloud.example/api/v1/tasks?project_id=proj_1&status=running&cursor=cur_1");
    expect(String(fetcher.mock.calls[1][0])).toBe("https://cloud.example/api/v1/runs/run_1/messages?after=41");
    expect(String(fetcher.mock.calls[2][0])).toBe("https://cloud.example/api/v1/runs/run_1");
    expect(String(fetcher.mock.calls[3][0])).toBe("https://cloud.example/api/v1/runs/run_1/usage");
    expect(String(fetcher.mock.calls[4][0])).toBe("https://cloud.example/api/v1/operations");
    expect(fetcher.mock.calls.flat().join(" ")).not.toContain("super-secret");
  });
});
