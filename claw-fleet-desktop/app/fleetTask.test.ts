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
