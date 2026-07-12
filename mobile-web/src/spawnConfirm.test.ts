import { describe, expect, it } from "vitest";
import { waitForSessionId } from "./spawnConfirm";
import type { SessionInfo } from "./types";

// 只需要 id 字段参与匹配，其余用 as 补齐避免造整个 SessionInfo。
function sess(id: string): SessionInfo {
  return { id } as SessionInfo;
}

describe("waitForSessionId", () => {
  it("快照里已有该 id 时立即成功，不 sleep", async () => {
    let slept = 0;
    const ok = await waitForSessionId("abc", () => [sess("x"), sess("abc")], {
      sleep: async () => void slept++,
    });
    expect(ok).toBe(true);
    expect(slept).toBe(0);
  });

  it("id 在几轮轮询后才出现 → 成功", async () => {
    let snapshot: SessionInfo[] = [];
    let ticks = 0;
    const ok = await waitForSessionId("late", () => snapshot, {
      attempts: 10,
      sleep: async () => {
        ticks++;
        if (ticks === 3) snapshot = [sess("late")]; // 第 3 轮桌面快照才带上会话
      },
    });
    expect(ok).toBe(true);
    expect(ticks).toBe(3);
  });

  it("宽限期内始终不出现 → 失败，且耗尽 attempts", async () => {
    let ticks = 0;
    const ok = await waitForSessionId("never", () => [sess("other")], {
      attempts: 5,
      sleep: async () => void ticks++,
    });
    expect(ok).toBe(false);
    expect(ticks).toBe(5);
  });
});
