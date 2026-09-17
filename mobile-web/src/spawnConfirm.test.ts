import { describe, expect, it } from "vitest";
import { waitForSessionId } from "./spawnConfirm";
import type { SessionInfo } from "./types";

// Only the id field participates in matching; use 'as' to fill the rest and avoid constructing the whole SessionInfo.
function sess(id: string): SessionInfo {
  return { id } as SessionInfo;
}

describe("waitForSessionId", () => {
  it("succeeds immediately when the id is already in the snapshot, no sleep", async () => {
    let slept = 0;
    const ok = await waitForSessionId("abc", () => [sess("x"), sess("abc")], {
      sleep: async () => void slept++,
    });
    expect(ok).toBe(true);
    expect(slept).toBe(0);
  });

  it("succeeds when the id appears after several polling rounds", async () => {
    let snapshot: SessionInfo[] = [];
    let ticks = 0;
    const ok = await waitForSessionId("late", () => snapshot, {
      attempts: 10,
      sleep: async () => {
        ticks++;
        if (ticks === 3) snapshot = [sess("late")]; // Session appears in desktop snapshot on the 3rd round
      },
    });
    expect(ok).toBe(true);
    expect(ticks).toBe(3);
  });

  it("fails when the id never appears within the grace period, exhausts all attempts", async () => {
    let ticks = 0;
    const ok = await waitForSessionId("never", () => [sess("other")], {
      attempts: 5,
      sleep: async () => void ticks++,
    });
    expect(ok).toBe(false);
    expect(ticks).toBe(5);
  });
});
