import { describe, expect, it } from "vitest";
import { recordSnapshotSource, MAX_SNAPSHOT_SOURCES } from "./snapshotSources";
import { agentKeyOf } from "./decisionReconcile";

const desk = { host: "mbp", pid: 18953, home: "/Users/hoveychen", ver: "0.0.0" };
const ghost = { host: "mbp", pid: 40404, home: "/tmp/t1", ver: "0.0.0" };

describe("recordSnapshotSource", () => {
  it("同一个 agent 反复回快照：合并成一条，累计次数并更新 lastAt", () => {
    let log = recordSnapshotSource([], {
      key: agentKeyOf(desk),
      agent: desk,
      at: 1_000,
      trusted: true,
      ignored: false,
    });
    log = recordSnapshotSource(log, {
      key: agentKeyOf(desk),
      agent: desk,
      at: 5_000,
      trusted: true,
      ignored: false,
    });
    expect(log).toHaveLength(1);
    expect(log[0].snapshots).toBe(2);
    expect(log[0].firstAt).toBe(1_000);
    expect(log[0].lastAt).toBe(5_000);
    expect(log[0].ignored).toBe(0);
  });

  it("换了个 agent：另起一条，并记下它的空快照被忽略过几次", () => {
    let log = recordSnapshotSource([], {
      key: agentKeyOf(desk),
      agent: desk,
      at: 1_000,
      trusted: true,
      ignored: false,
    });
    log = recordSnapshotSource(log, {
      key: agentKeyOf(ghost),
      agent: ghost,
      at: 2_000,
      trusted: false,
      ignored: true,
    });
    expect(log).toHaveLength(2);
    const g = log.find((s) => s.agent?.pid === 40404)!;
    expect(g.ignored).toBe(1);
    expect(g.trusted).toBe(false);
    // 主 agent 那条不受影响 —— 这正是「谁是李鬼」的判据。
    expect(log.find((s) => s.agent?.pid === 18953)!.trusted).toBe(true);
  });

  it("没有指纹的老桌面端：归到一条匿名来源，不和有指纹的混在一起", () => {
    let log = recordSnapshotSource([], {
      key: undefined,
      agent: undefined,
      at: 1_000,
      trusted: false,
      ignored: false,
    });
    log = recordSnapshotSource(log, {
      key: undefined,
      agent: undefined,
      at: 2_000,
      trusted: false,
      ignored: true,
    });
    expect(log).toHaveLength(1);
    expect(log[0].key).toBeUndefined();
    expect(log[0].snapshots).toBe(2);
    expect(log[0].ignored).toBe(1);
  });

  it("来源数量有上限：只保留最近活跃的若干条，不无限增长", () => {
    let log: ReturnType<typeof recordSnapshotSource> = [];
    for (let i = 0; i < MAX_SNAPSHOT_SOURCES + 3; i++) {
      log = recordSnapshotSource(log, {
        key: `agent-${i}`,
        agent: { ...ghost, pid: i },
        at: 1_000 + i,
        trusted: false,
        ignored: false,
      });
    }
    expect(log).toHaveLength(MAX_SNAPSHOT_SOURCES);
    // 最老的被挤掉，最新的一定在。
    expect(log.some((s) => s.key === "agent-0")).toBe(false);
    expect(log.some((s) => s.key === `agent-${MAX_SNAPSHOT_SOURCES + 2}`)).toBe(true);
  });
});
