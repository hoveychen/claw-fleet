import { beforeEach, describe, expect, it } from "vitest";
import type { SessionInfo, SessionStatus } from "../types";
import { resetQuietAliveLatch } from "../types";
import {
  groupSessionsByStatus,
  statusBucketOf,
  statusSectionPath,
} from "./sessionStatusGroups";

function session(
  id: string,
  status: SessionStatus,
  agentLastActivityMs: number,
  procAlive = false,
): SessionInfo {
  return {
    id,
    jsonlPath: `/sessions/${id}.jsonl`,
    workspacePath: "/work/manta",
    workspaceName: "manta",
    status,
    procAlive,
    lastActivityMs: agentLastActivityMs,
    agentLastActivityMs,
  } as SessionInfo;
}

describe("statusBucketOf", () => {
  beforeEach(resetQuietAliveLatch);

  it("separates the attention states from ended work", () => {
    expect(statusBucketOf(session("a", "executing", 1))).toBe("running");
    expect(statusBucketOf(session("b", "waitingInput", 1))).toBe("waitingInput");
    expect(statusBucketOf(session("c", "watching", 1))).toBe("watching");
    expect(statusBucketOf(session("d", "idle", 1))).toBe("ended");
  });

  it("keeps a wedged or cut-off session out of the ended bucket", () => {
    // These write nothing while their process lives, so a plain liveness test
    // would file them under ended — the one state that always needs a human.
    expect(statusBucketOf(session("limited", "rateLimited", 1))).toBe("error");
    expect(statusBucketOf(session("wedged", "stuck", 1, true))).toBe("error");
    expect(statusBucketOf(session("cut", "remoteDisconnected", 1))).toBe("error");
  });

  it("keeps a quiet-but-alive session under running", () => {
    // Parked on one long tool call: the status decayed to idle while the CLI
    // process is very much alive, which is the faded-green dot on the row.
    const quiet = session("slow-build", "idle", Date.now(), true);
    expect(statusBucketOf(quiet)).toBe("running");
  });
});

describe("groupSessionsByStatus", () => {
  beforeEach(resetQuietAliveLatch);

  it("orders sections by attention and rows by activity, dropping empty buckets", () => {
    const groups = groupSessionsByStatus([
      session("done-old", "idle", 10),
      session("parked", "waitingInput", 20),
      session("done-new", "idle", 30),
      session("busy", "thinking", 40),
    ]);

    expect(groups.map((g) => g.bucket)).toEqual([
      "running",
      "waitingInput",
      "ended",
    ]);
    expect(groups[0].path).toBe(statusSectionPath("running"));
    expect(groups[2].sessions.map((s) => s.id)).toEqual(["done-new", "done-old"]);
  });

  it("leaves rows in the caller's order under preserveOrder", () => {
    const groups = groupSessionsByStatus(
      [session("older", "idle", 10), session("newer", "idle", 99)],
      { preserveOrder: true },
    );

    expect(groups[0].sessions.map((s) => s.id)).toEqual(["older", "newer"]);
  });
});
