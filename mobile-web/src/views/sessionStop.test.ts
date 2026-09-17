import { describe, expect, it, vi } from "vitest";
import { canControl, runStop, stopMode } from "./sessionStop";
import type { SessionInfo } from "../types";
import type { FleetTransport } from "../transport";

const base: SessionInfo = {
  id: "abc-123",
  workspacePath: "/Users/x/workspace/proj",
  workspaceName: "proj",
  status: "executing",
  isSubagent: false,
  lastActivityMs: 0,
  createdAtMs: 0,
  jsonlPath: "/Users/x/.claude/projects/proj/abc-123.jsonl",
};

const fleetOwned = { ...base, pid: 42, pidPrecise: true, entrypoint: "claw-fleet-newsession" };

function fakeTransport() {
  const calls: Array<{ method: string; params: unknown }> = [];
  const transport = {
    request: vi.fn(async (method: string, params: unknown) => {
      calls.push({ method, params });
      return null;
    }),
  } as unknown as FleetTransport;
  return { transport, calls };
}

describe("stopMode", () => {
  it("no pid = process already gone, can't stop", () => {
    expect(stopMode(base)).toBe("spent");
  });

  it("Fleet-spawned session, precise pid, executing turn → interrupt (resumable), not kill", () => {
    expect(stopMode(fleetOwned)).toBe("interrupt");
  });

  it("same session becomes idle, no turn left to interrupt, downgrade to stop", () => {
    expect(stopMode({ ...fleetOwned, status: "idle" })).toBe("stop");
  });

  it("not Fleet-spawned or pid not precise, can only stop", () => {
    expect(stopMode({ ...fleetOwned, entrypoint: "cli" })).toBe("stop");
    expect(stopMode({ ...fleetOwned, pidPrecise: false })).toBe("stop");
  });
});

describe("canControl", () => {
  it("subagent has no process of its own, no button", () => {
    expect(canControl({ ...fleetOwned, isSubagent: true })).toBe(false);
    expect(canControl(fleetOwned)).toBe(true);
  });
});

describe("runStop", () => {
  const yes = (_msg: string) => Promise.resolve(true);
  const no = (_msg: string) => Promise.resolve(false);

  it("interrupt doesn't ask for confirmation — doesn't kill process, turn can continue", async () => {
    const { transport, calls } = fakeTransport();
    const confirm = vi.fn(yes);
    await expect(runStop(transport, fleetOwned, confirm)).resolves.toBe(true);
    expect(confirm).not.toHaveBeenCalled();
    expect(calls).toEqual([{ method: "interrupt", params: { pid: 42 } }]);
  });

  it("precise-pid stop asks for confirmation once, hits by pid", async () => {
    const { transport, calls } = fakeTransport();
    const s = { ...fleetOwned, status: "idle" as const };
    await expect(runStop(transport, s, yes)).resolves.toBe(true);
    expect(calls).toEqual([{ method: "stop", params: { pid: 42 } }]);
  });

  it("imprecise pid stops whole workspace, confirmation explains that clearly", async () => {
    const { transport, calls } = fakeTransport();
    const s = { ...fleetOwned, pidPrecise: false };
    const confirm = vi.fn(yes);
    await expect(runStop(transport, s, confirm)).resolves.toBe(true);
    // Test environment's t() uses English, so we match either form — key is this
    // confirmation must say "all sessions in the workspace will stop", not reuse
    // the text for stopping just one session.
    expect(confirm.mock.calls[0][0]).toMatch(/目录下的所有会话|ALL sessions/);
    expect(calls).toEqual([
      { method: "stop_workspace", params: { workspacePath: "/Users/x/workspace/proj" } },
    ]);
  });

  it("user cancels on confirmation prompt → send no requests", async () => {
    const { transport, calls } = fakeTransport();
    const s = { ...fleetOwned, status: "idle" as const };
    await expect(runStop(transport, s, no)).resolves.toBe(false);
    expect(calls).toEqual([]);
  });

  it("session with no process sends no request", async () => {
    const { transport, calls } = fakeTransport();
    await expect(runStop(transport, base, yes)).resolves.toBe(false);
    expect(calls).toEqual([]);
  });
});
