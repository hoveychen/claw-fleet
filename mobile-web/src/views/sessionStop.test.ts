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
  it("没有 pid = 进程已经没了，无从停起", () => {
    expect(stopMode(base)).toBe("spent");
  });

  it("Fleet 起的会话、pid 精确、正跑在回合中 → 中断（可继续），不是杀掉", () => {
    expect(stopMode(fleetOwned)).toBe("interrupt");
  });

  it("同一条会话空闲下来之后就没有回合可打断了，降级成停止", () => {
    expect(stopMode({ ...fleetOwned, status: "idle" })).toBe("stop");
  });

  it("不是 Fleet 起的、或 pid 不精确，都只能停止", () => {
    expect(stopMode({ ...fleetOwned, entrypoint: "cli" })).toBe("stop");
    expect(stopMode({ ...fleetOwned, pidPrecise: false })).toBe("stop");
  });
});

describe("canControl", () => {
  it("子代理没有自己的进程，不给按钮", () => {
    expect(canControl({ ...fleetOwned, isSubagent: true })).toBe(false);
    expect(canControl(fleetOwned)).toBe(true);
  });
});

describe("runStop", () => {
  const yes = (_msg: string) => Promise.resolve(true);
  const no = (_msg: string) => Promise.resolve(false);

  it("中断不问确认——它不杀进程，回合还能接着跑", async () => {
    const { transport, calls } = fakeTransport();
    const confirm = vi.fn(yes);
    await expect(runStop(transport, fleetOwned, confirm)).resolves.toBe(true);
    expect(confirm).not.toHaveBeenCalled();
    expect(calls).toEqual([{ method: "interrupt", params: { pid: 42 } }]);
  });

  it("pid 精确的停止要确认一次，按 pid 打", async () => {
    const { transport, calls } = fakeTransport();
    const s = { ...fleetOwned, status: "idle" as const };
    await expect(runStop(transport, s, yes)).resolves.toBe(true);
    expect(calls).toEqual([{ method: "stop", params: { pid: 42 } }]);
  });

  it("pid 不精确时改成停整个目录，且说清楚了才问", async () => {
    const { transport, calls } = fakeTransport();
    const s = { ...fleetOwned, pidPrecise: false };
    const confirm = vi.fn(yes);
    await expect(runStop(transport, s, confirm)).resolves.toBe(true);
    // 测试环境的 t() 走英文，所以两种文案都认——要点是这句确认必须说出「整个
    // 目录都会被停」，而不是复用那句只停一条会话的。
    expect(confirm.mock.calls[0][0]).toMatch(/目录下的所有会话|ALL sessions/);
    expect(calls).toEqual([
      { method: "stop_workspace", params: { workspacePath: "/Users/x/workspace/proj" } },
    ]);
  });

  it("用户在确认框上取消 → 一个请求都不发", async () => {
    const { transport, calls } = fakeTransport();
    const s = { ...fleetOwned, status: "idle" as const };
    await expect(runStop(transport, s, no)).resolves.toBe(false);
    expect(calls).toEqual([]);
  });

  it("已经没进程的会话不发请求", async () => {
    const { transport, calls } = fakeTransport();
    await expect(runStop(transport, base, yes)).resolves.toBe(false);
    expect(calls).toEqual([]);
  });
});
