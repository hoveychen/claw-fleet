import { describe, expect, it, vi } from "vitest";
import { RelayClientAdapter } from "../RelayClientAdapter";

describe("RelayClientAdapter", () => {
  it("maps existing relay snapshots into the FleetDataClient task shape", async () => {
    const relay = { request: vi.fn(), answer: vi.fn() };
    const adapter = new RelayClientAdapter(relay as never, () => ({
      decisions: [],
      sessions: [
        {
          id: "session_1",
          workspacePath: "/repo",
          workspaceName: "fleet",
          status: "active",
          procAlive: true,
          lastActivityMs: 1_700_000_000_000,
          jsonlPath: "/private/transcript.jsonl",
          isSubagent: false,
        } as never,
      ],
    }));
    const page = await adapter.listTasks({});
    expect(page.data[0]).toMatchObject({
      id: "session_1",
      project_id: "/repo",
      status: "running",
      active_run_id: "session_1",
    });
  });
});
