// "Just woken" latch (shared-ts/wakeLatch.ts) and its effect on the home's
// five-state grouping. The snapshot sequences replay what an isolated
// `fleet webui` reported on 2026-10-19 while a watch woke a coordinator.
import { beforeEach, describe, expect, it } from "vitest";
import {
  WAKE_HOLD_MS,
  createWakeLatch,
  stickyWaking,
  type WakeObservation,
} from "../../shared-ts/wakeLatch";
import { taskStateOf } from "../../shared-ts/taskState";
import { buildHomeRows } from "./components/homeRows";
import {
  NEW_SESSION_ENTRYPOINT,
  isWakingSticky,
  resetWakingLatch,
  type PendingDecision,
  type SessionInfo,
  type SessionStatus,
} from "./types";

const obs = (o: Partial<WakeObservation>): WakeObservation => ({
  parked: false,
  alive: false,
  lastActivityMs: 1000,
  now: 0,
  ...o,
});

describe("stickyWaking", () => {
  it("arms when a parked session is let go and holds until the transcript moves", () => {
    const st = createWakeLatch();
    expect(stickyWaking(st, "s", obs({ parked: true, now: 0 }))).toBe(false);
    expect(stickyWaking(st, "s", obs({ parked: false, now: 100 }))).toBe(true);
    expect(stickyWaking(st, "s", obs({ parked: false, now: 200 }))).toBe(true);
    expect(stickyWaking(st, "s", obs({ parked: false, lastActivityMs: 2000, now: 300 }))).toBe(false);
  });

  it("arms when the process comes back", () => {
    const st = createWakeLatch();
    stickyWaking(st, "s", obs({ alive: false, now: 0 }));
    expect(stickyWaking(st, "s", obs({ alive: true, now: 50 }))).toBe(true);
  });

  it("gives up after the hold window", () => {
    const st = createWakeLatch();
    stickyWaking(st, "s", obs({ parked: true, now: 0 }));
    expect(stickyWaking(st, "s", obs({ now: 10 }))).toBe(true);
    expect(stickyWaking(st, "s", obs({ now: 10 + WAKE_HOLD_MS }))).toBe(false);
  });

  it("never arms from a single snapshot, so a stale alive reading is left alone", () => {
    const st = createWakeLatch();
    expect(stickyWaking(st, "s", obs({ alive: true, now: 0 }))).toBe(false);
    expect(stickyWaking(st, "s", obs({ alive: true, now: 100 }))).toBe(false);
  });

  it("a new card disarms it", () => {
    const st = createWakeLatch();
    stickyWaking(st, "s", obs({ parked: true }));
    stickyWaking(st, "s", obs({}));
    expect(stickyWaking(st, "s", obs({ parked: true }))).toBe(false);
  });
});

describe("taskStateOf with waking", () => {
  it("a waking main hop reads as running even at the old waitingInput", () => {
    const m = { status: "waitingInput", procAlive: true, quiet: false, hasPendingDecision: false };
    expect(taskStateOf({ main: [m], subtasks: [], outcome: null })).toBe("review");
    expect(taskStateOf({ main: [{ ...m, waking: true }], subtasks: [], outcome: null })).toBe("running");
  });
});

function session(id: string, over: Partial<SessionInfo> = {}): SessionInfo {
  return {
    id,
    jsonlPath: `/sessions/${id}.jsonl`,
    workspacePath: "/work/fleet",
    workspaceName: "fleet",
    status: "idle" as SessionStatus,
    procAlive: false,
    entrypoint: NEW_SESSION_ENTRYPOINT,
    fleetSpawned: true,
    isSubagent: false,
    createdAtMs: 100,
    lastActivityMs: 1000,
    agentLastActivityMs: 1000,
    aiTitle: `title ${id}`,
    taskId: id,
    ...over,
  } as SessionInfo;
}

function card(id: string, sessionId: string): PendingDecision {
  return {
    kind: "fleet-ask",
    id,
    request: { id, sessionId, workspaceName: "fleet", aiTitle: "t", questions: [] },
    answers: {},
    arrivedAt: 0,
  } as unknown as PendingDecision;
}

const opts = {
  isQuiet: () => false,
  isWaking: (s: SessionInfo, pending: boolean) => isWakingSticky(s, pending),
  untitled: "untitled",
};
const stateOf = (sessions: SessionInfo[], decisions: PendingDecision[] = []) =>
  buildHomeRows(sessions, decisions, opts).find((r) => r.kind === "task")!.state;

describe("buildHomeRows across a wake-up", () => {
  beforeEach(() => resetWakingLatch());

  it("a watch-woken coordinator never passes through review", () => {
    const watching = { status: "watching" as SessionStatus, watches: [{}] } as Partial<SessionInfo>;
    expect(stateOf([session("p", watching)])).toBe("waiting");
    // The watch fired and its resume is up, but nothing is written yet.
    expect(
      stateOf([session("p", { status: "waitingInput" as SessionStatus, procAlive: true, watches: [] })]),
    ).toBe("running");
    expect(stateOf([session("p", { status: "thinking" as SessionStatus, procAlive: true, lastActivityMs: 2000 })]))
      .toBe("running");
    // The resumed turn ends: now it is genuinely ready to accept.
    expect(stateOf([session("p", { status: "waitingInput" as SessionStatus, lastActivityMs: 3000 })])).toBe(
      "review",
    );
  });

  it("an answered card holds the task out of review until the transcript moves", () => {
    const parked = session("p", { status: "waitingInput" as SessionStatus });
    expect(stateOf([parked], [card("c", "p")])).toBe("needsYou");
    expect(stateOf([parked])).toBe("running");
    expect(stateOf([session("p", { status: "waitingInput" as SessionStatus, lastActivityMs: 2000 })])).toBe(
      "review",
    );
  });
});
