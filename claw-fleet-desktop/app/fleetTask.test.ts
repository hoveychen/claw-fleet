import { describe, expect, it } from "vitest";
import {
  isFleetOwnedTask,
  NEW_SESSION_ENTRYPOINT,
  type SessionInfo,
} from "./types";

function session(over: Partial<SessionInfo> = {}): SessionInfo {
  return {
    entrypoint: NEW_SESSION_ENTRYPOINT,
    isSubagent: false,
    fleetSpawned: true,
    ...over,
  } as SessionInfo;
}

describe("isFleetOwnedTask", () => {
  // dsh regression. dsh has no originator channel of its own — every session
  // runs inside one shared `dsh web`, so there is no per-session process to
  // stamp an entrypoint on. Its SessionInfo therefore comes back
  // `entrypoint: null, fleetSpawned: false` unless the spawn path recorded a
  // launch note and the source mapping backfilled both fields.
  //
  // It didn't, and the cost was invisible from the backend: the session was
  // created, `dsh web` was healthy, every live test passed — but the
  // "新建会话" dialog waits for the spawned id to appear in
  // `sessions.filter(isFleetOwnedTask)`, so it sat on "正在启动会话…" forever.
  // These two cases pin the predicate that decides it.
  it("includes a dsh session Fleet spawned (both fields backfilled)", () => {
    expect(
      isFleetOwnedTask(
        session({
          agentSource: "dsh",
          entrypoint: NEW_SESSION_ENTRYPOINT,
          fleetSpawned: true,
        }),
      ),
    ).toBe(true);
  });

  it("excludes a dsh session the user started outside Fleet", () => {
    // What an unrecorded dsh session actually looks like — measured on the
    // installed build before the fix.
    expect(
      isFleetOwnedTask(
        session({ agentSource: "dsh", entrypoint: null, fleetSpawned: false }),
      ),
    ).toBe(false);
  });

  it("includes a Fleet-owned session Fleet actually spawned", () => {
    expect(isFleetOwnedTask(session({ fleetSpawned: true }))).toBe(true);
  });

  it("excludes a leaked claude -p child: Fleet entrypoint but no spawn marker", () => {
    // The whole bug: a `claude -p` run inside a Fleet session inherits
    // CLAUDE_CODE_ENTRYPOINT and looks Fleet-owned, but fleetSpawned is false.
    expect(isFleetOwnedTask(session({ fleetSpawned: false }))).toBe(false);
  });

  it("excludes subagents even when spawned by Fleet", () => {
    expect(
      isFleetOwnedTask(session({ isSubagent: true, fleetSpawned: true })),
    ).toBe(false);
  });

  it("excludes externally-started sessions (non-Fleet entrypoint)", () => {
    expect(
      isFleetOwnedTask(session({ entrypoint: "cli", fleetSpawned: false })),
    ).toBe(false);
  });
});
