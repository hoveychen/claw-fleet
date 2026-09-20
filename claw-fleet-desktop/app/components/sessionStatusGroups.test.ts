import { beforeEach, describe, expect, it } from "vitest";
import type { SessionInfo, SessionStatus } from "../types";
import { resetQuietAliveLatch } from "../types";
import {
  groupItemsByStatus,
  itemBucketOf,
  statusBucketOf,
  statusSectionPath,
} from "./sessionStatusGroups";
import { buildRenderItems } from "./sessionGroups";

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

/** A session that is hop `hop` of a `chainLen`-long relay chain. */
function hop(
  id: string,
  status: SessionStatus,
  agentLastActivityMs: number,
  hopNo: number,
  procAlive = false,
): SessionInfo {
  return {
    ...session(id, status, agentLastActivityMs, procAlive),
    handoff: { chainId: "relay-1", chainLen: 3, hop: hopNo },
  } as SessionInfo;
}

/** The rail's own pipeline: fold chains, then partition by status. */
function group(rows: SessionInfo[], opts?: { preserveOrder?: boolean }) {
  return groupItemsByStatus(buildRenderItems(rows, true), opts);
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

function idsOf(items: ReturnType<typeof group>[number]["items"]): string[] {
  return items.map((it) => (it.kind === "single" ? it.session.id : it.chainId));
}

describe("groupItemsByStatus", () => {
  beforeEach(resetQuietAliveLatch);

  it("orders sections by attention and rows by activity, dropping empty buckets", () => {
    const groups = group([
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
    expect(idsOf(groups[2].items)).toEqual(["done-new", "done-old"]);
  });

  it("leaves rows in the caller's order under preserveOrder", () => {
    const groups = group([session("older", "idle", 10), session("newer", "idle", 99)], {
      preserveOrder: true,
    });

    expect(idsOf(groups[0].items)).toEqual(["older", "newer"]);
  });

  it("keeps a live relay chain whole under running", () => {
    // Hops that have handed off have no process and a decayed status, so
    // bucketing them individually filed the chain under "ended" alongside its
    // own running tip.
    const groups = group([
      hop("tip", "thinking", 40, 3, true),
      hop("retired-2", "idle", 30, 2),
      hop("retired-1", "idle", 20, 1),
      session("plain-done", "idle", 10),
    ]);

    expect(groups.map((g) => g.bucket)).toEqual(["running", "ended"]);
    expect(idsOf(groups[0].items)).toEqual(["relay-1"]);
    expect(idsOf(groups[1].items)).toEqual(["plain-done"]);
  });

  it("files a fully retired chain under ended", () => {
    const groups = group([hop("last", "idle", 30, 3), hop("first", "idle", 20, 1)]);

    expect(groups.map((g) => g.bucket)).toEqual(["ended"]);
    expect(idsOf(groups[0].items)).toEqual(["relay-1"]);
  });

  it("sorts a chain by its most recent hop", () => {
    const groups = group([
      session("solo", "idle", 25),
      hop("tip", "idle", 30, 3),
      hop("first", "idle", 5, 1),
    ]);

    expect(idsOf(groups[0].items)).toEqual(["relay-1", "solo"]);
  });
});

describe("itemBucketOf", () => {
  beforeEach(resetQuietAliveLatch);

  it("takes the most salient hop of a chain", () => {
    const [chain] = buildRenderItems(
      [hop("tip", "idle", 30, 3), hop("parked", "waitingInput", 20, 2)],
      true,
    );
    expect(itemBucketOf(chain)).toBe("waitingInput");
  });
});
