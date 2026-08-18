// The "新建会话" dialog swaps its spinner for the session view only once the id
// it just spawned turns up in `sessions.filter(isFleetOwnedTask)`. That wait had
// no test, and it is exactly where a dsh spawn hung: the session existed and
// `dsh web` was healthy, but its SessionInfo came back
// `entrypoint: null, fleetSpawned: false`, so it never entered the filtered list
// the dialog was watching. Nothing in the logs said so — the spinner just spun.
//
// These cases pin the matcher against the *filtered* list, which is the shape it
// is actually called with.
import { describe, expect, it } from "vitest";

import { matchSpawnedSession, type PendingSpawn } from "./HistoryView";
import { isFleetOwnedTask, NEW_SESSION_ENTRYPOINT, type SessionInfo } from "../types";

function session(over: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id: "s1",
    workspacePath: "/ws",
    entrypoint: NEW_SESSION_ENTRYPOINT,
    isSubagent: false,
    fleetSpawned: true,
    createdAtMs: 1000,
    ...over,
  } as SessionInfo;
}

function pending(over: Partial<PendingSpawn> = {}): PendingSpawn {
  return {
    workspacePath: "/ws",
    knownIds: new Set<string>(),
    ...over,
  } as PendingSpawn;
}

describe("matchSpawnedSession", () => {
  it("finds the spawned session by the id the spawn returned", () => {
    const rows = [session({ id: "other" }), session({ id: "want" })];
    const hit = matchSpawnedSession(rows, pending({ sessionId: "want" }));
    expect(hit?.id).toBe("want");
  });

  it("never matches a session that isFleetOwnedTask filtered out — the hang", () => {
    // A dsh session as it came back BEFORE the fix. The dialog filters with
    // isFleetOwnedTask first, so this row is not even in the list it searches;
    // the matcher then returns undefined forever and the spinner never stops.
    const unowned = session({
      id: "dsh-1",
      agentSource: "dsh",
      entrypoint: null,
      fleetSpawned: false,
    });
    expect(isFleetOwnedTask(unowned)).toBe(false);

    const filtered = [unowned].filter(isFleetOwnedTask);
    expect(matchSpawnedSession(filtered, pending({ sessionId: "dsh-1" }))).toBeUndefined();
  });

  it("resolves once the spawn marker backfills both fields — the fix", () => {
    const owned = session({
      id: "dsh-1",
      agentSource: "dsh",
      entrypoint: NEW_SESSION_ENTRYPOINT,
      fleetSpawned: true,
    });
    const filtered = [owned].filter(isFleetOwnedTask);
    expect(matchSpawnedSession(filtered, pending({ sessionId: "dsh-1" }))?.id).toBe("dsh-1");
  });

  it("falls back to the newest unseen session in the workspace when no id came back", () => {
    // The remote-probe path returns only a pid, so the id is unknown.
    const rows = [
      session({ id: "old", createdAtMs: 10 }),
      session({ id: "new", createdAtMs: 99 }),
      session({ id: "elsewhere", workspacePath: "/other", createdAtMs: 100 }),
    ];
    const hit = matchSpawnedSession(rows, pending({ knownIds: new Set(["old"]) }));
    expect(hit?.id).toBe("new");
  });
});
