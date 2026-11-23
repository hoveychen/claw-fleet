/**
 * `?mock` stand-in for the machine-global resource queue
 * (`claw_fleet_core::resource_queue`). Keeps a small table in memory and runs
 * the same scheduling rule as `State::plan` so a hand reorder changes who waits
 * on what exactly as it would for real.
 *
 * Every handler answers after a delay: the real calls take the queue's file
 * lock (and `kick` waits for the holder to let go), and an instant mock hides
 * the races between the panel's poll and its own writes.
 *
 * Every transition is also appended to an in-memory event log, seeded with
 * an hour of finished work, so the history section has something to show.
 *
 * `?mock&resources=empty` starts with an empty table and no history.
 */
import type {
  HistoryQuery,
  ResourceEvent,
  ResourceEventKind,
  MoveTarget,
  ProcRecord,
  QueuedRequest,
  ResourceRequest,
  ResourceSnapshot,
  ResourceView,
  WaitOn,
} from "../generated/types";

const IPC_DELAY_MS = 120;
const KICK_DELAY_MS = 700;

interface MockState {
  capacity: Record<string, number>;
  grants: ResourceRequest[];
  queue: ResourceRequest[];
  history: ResourceEvent[];
}

function ev(kind: ResourceEventKind, r: ResourceRequest, tsMs: number, extra: Partial<ResourceEvent> = {}): ResourceEvent {
  return {
    tsMs,
    kind,
    requestId: r.id,
    requestKind: r.kind,
    needs: r.needs,
    priority: r.priority,
    pid: r.holder.pid,
    sessionId: r.sessionId,
    command: r.command || undefined,
    workspace: r.workspace,
    ...extra,
  };
}

/** A finished request: queued, granted after `waitMs`, then ended by `end`
 *  after `holdMs`. Returns its events oldest first. */
function finished(
  r: ResourceRequest,
  waitMs: number,
  holdMs: number,
  end: ResourceEventKind,
  exitCode?: number,
): ResourceEvent[] {
  const t0 = r.enqueuedMs;
  const out = [ev("enqueued", r, t0)];
  if (end === "cancelled" || end === "timed_out") {
    out.push(ev(end, r, t0 + waitMs, { waitedMs: waitMs }));
    return out;
  }
  out.push(ev("granted", r, t0 + waitMs, { waitedMs: waitMs }));
  out.push(ev(end, r, t0 + waitMs + holdMs, { heldMs: holdMs, exitCode }));
  return out;
}

function seedHistory(s: Omit<MockState, "history">): ResourceEvent[] {
  const now = Date.now();
  const min = 60_000;
  const old = (id: string, needs: Record<string, number>, agoMs: number, command: string, extra: Partial<ResourceRequest> = {}) =>
    req(id, needs, agoMs, command, extra);
  const past: ResourceEvent[] = [
    { tsMs: now - 62 * min, kind: "capacity_set", resource: "cargo", capacity: 2, prevCapacity: 1, needs: {} },
    ...finished(old("r19c49e011", { cargo: 1 }, 58 * min, "cargo test -p fleet-cli --test resource_queue_e2e", { kind: "foreground", sessionId: "2b7c9e41-5f0d-4c61-9a8e-0d3f6e2a7b10" }), 0, 3 * min + 12_000, "released", 0),
    ...finished(old("p1a1fb0c2d11", { "test-db": 1 }, 51 * min, "pnpm test:db --runInBand", { sessionId: "8e1d0c55-3a2b-4f7e-b6a1-77c2e9d04f3a" }), 2_000, 6 * min, "released", 1),
    ...finished(old("p1a1fb1e9a02", { gpu: 1 }, 44 * min, "python train.py --epochs 20", { sessionId: "f2a8b6d1-0c94-4e57-8b3a-61d9e5c27f08", workspace: "/Users/demo/workspace/vector-lab" }), 0, 21 * min, "kicked", 143),
    ...finished(old("r19c49f3b7", { cargo: 1 }, 30 * min, "cargo build --release", { kind: "foreground" }), 4 * min, 0, "timed_out"),
    ...finished(old("p1a1fb2f0e55", { "e2e-env": 1, "test-db": 1 }, 25 * min, "pnpm e2e --project mobile", { sessionId: "c4f09a12-6e3d-4b8a-9f21-5a0b7d3e8c64" }), 90_000, 7 * min, "reaped"),
    ...finished(old("p1a1fb3aa871", { gpu: 1 }, 18 * min, "python eval.py --suite small"), 40_000, 0, "cancelled"),
  ];
  const live: ResourceEvent[] = [];
  for (const g of s.grants) {
    live.push(ev("enqueued", g, g.enqueuedMs));
    live.push(ev("granted", g, g.grantedMs ?? g.enqueuedMs, { waitedMs: (g.grantedMs ?? g.enqueuedMs) - g.enqueuedMs }));
  }
  for (const q of s.queue) live.push(ev("enqueued", q, q.enqueuedMs));
  return [...past, ...live].sort((a, b) => a.tsMs - b.tsMs);
}

/** `read_history_at`: newest first, filtered, one page. */
function readHistory(s: MockState, q: HistoryQuery) {
  const limit = Math.min(Math.max(q.limit ?? 200, 1), 2000);
  // Chronological, then newest first: ties keep append order reversed, as core does.
  const all = [...s.history]
    .sort((a, b) => a.tsMs - b.tsMs)
    .reverse()
    .filter(
      (e) =>
        (q.beforeMs == null || e.tsMs < q.beforeMs) &&
        (!q.resource || e.resource === q.resource || q.resource in (e.needs ?? {})) &&
        (!q.requestId || e.requestId === q.requestId),
    );
  return { events: all.slice(0, limit), hasMore: all.length > limit };
}

let state: MockState | null = null;

function req(
  id: string,
  needs: Record<string, number>,
  agoMs: number,
  command: string,
  extra: Partial<ResourceRequest> = {},
): ResourceRequest {
  const now = Date.now();
  return {
    id,
    kind: "job",
    needs,
    priority: "normal",
    enqueuedMs: now - agoMs,
    manual: false,
    holder: { pid: 40000 + id.length },
    command,
    workspace: "/Users/demo/workspace/claude-fleet",
    ...extra,
  };
}

function seed(): MockState {
  const empty =
    typeof window !== "undefined" &&
    new URLSearchParams(window.location.search).get("resources") === "empty";
  if (empty) return { capacity: {}, grants: [], queue: [], history: [] };
  const now = Date.now();
  const table: Omit<MockState, "history"> = {
    capacity: { cargo: 2, "test-db": 1, gpu: 1, "e2e-env": 1 },
    grants: [
      req("r19c4a1f2e", { cargo: 1 }, 4 * 60_000, "cargo test -p claw-fleet-core --lib", {
        kind: "foreground",
        grantedMs: now - 3 * 60_000 - 12_000,
        sessionId: "2b7c9e41-5f0d-4c61-9a8e-0d3f6e2a7b10",
      }),
      req("r19c4a1f7d", { cargo: 1 }, 70_000, "cargo clippy --workspace --all-targets", {
        kind: "foreground",
        grantedMs: now - 52_000,
        sessionId: "9a3e5b70-1d2c-4e8f-a6b5-c0d1e2f3a4b5",
        workspace: "/Users/demo/workspace/claude-fleet/.worktrees/resource-queue-p2",
      }),
      req("p1a1fc3d8e44", { "test-db": 1, "e2e-env": 1 }, 9 * 60_000, "pnpm e2e --project desktop", {
        grantedMs: now - 8 * 60_000 - 40_000,
        sessionId: "8e1d0c55-3a2b-4f7e-b6a1-77c2e9d04f3a",
        workspace: "/Users/demo/workspace/claude-fleet/.worktrees/resource-queue-p4",
      }),
    ],
    queue: [
      req("p1a1fc40ef29", { "test-db": 1, gpu: 1 }, 6 * 60_000, "python scripts/bench_vectors.py --full", {
        sessionId: "c4f09a12-6e3d-4b8a-9f21-5a0b7d3e8c64",
        workspace: "/Users/demo/workspace/vector-lab",
      }),
      req("p1a1fc41a7b3", { gpu: 1 }, 2 * 60_000 + 30_000, "python train.py --epochs 3", {
        sessionId: "f2a8b6d1-0c94-4e57-8b3a-61d9e5c27f08",
        workspace: "/Users/demo/workspace/vector-lab",
      }),
      req("r19c4a2c07", { cargo: 1 }, 45_000, "cargo build -p claw-fleet-desktop", {
        kind: "foreground",
        sessionId: "2b7c9e41-5f0d-4c61-9a8e-0d3f6e2a7b10",
      }),
    ],
  };
  return { ...table, history: seedHistory(table) };
}

function st(): MockState {
  if (!state) state = seed();
  return state;
}

/** `State::plan`: grant in queue order unless a resource lacks free units or
 *  is reserved by a request further ahead that could not be granted. */
function plan(s: MockState): { grant: number[]; waits: Map<number, WaitOn[]> } {
  const used: Record<string, number> = {};
  for (const g of s.grants) for (const [n, u] of Object.entries(g.needs)) used[n] = (used[n] ?? 0) + (u ?? 0);
  const reserved: Record<string, string[]> = {};
  const grant: number[] = [];
  const waits = new Map<number, WaitOn[]>();
  s.queue.forEach((r, i) => {
    const blockers: WaitOn[] = [];
    for (const [name, units] of Object.entries(r.needs)) {
      const free = (s.capacity[name] ?? 1) - (used[name] ?? 0);
      if (free < (units ?? 0)) blockers.push({ resource: name, reason: "busy", by: [] });
      else if (reserved[name]) blockers.push({ resource: name, reason: "reserved", by: [...reserved[name]] });
    }
    if (blockers.length === 0) {
      for (const [n, u] of Object.entries(r.needs)) used[n] = (used[n] ?? 0) + (u ?? 0);
      grant.push(i);
    } else {
      for (const n of Object.keys(r.needs)) (reserved[n] ??= []).push(r.id);
      waits.set(i, blockers);
    }
  });
  return { grant, waits };
}

function schedule(s: MockState) {
  const { grant } = plan(s);
  const now = Date.now();
  const taken = grant.map((i) => s.queue[i]);
  s.queue = s.queue.filter((_, i) => !grant.includes(i));
  for (const r of taken) {
    s.grants.push({ ...r, grantedMs: now });
    s.history.push(ev("granted", r, now, { waitedMs: now - r.enqueuedMs }));
  }
}

function snapshot(s: MockState): ResourceSnapshot {
  schedule(s);
  const holdersOf = (name: string) => s.grants.filter((g) => name in g.needs).map((g) => g.id);
  const { waits } = plan(s);
  const queue: QueuedRequest[] = s.queue.map((r, i) => ({
    position: i + 1,
    request: r,
    waitOn: (waits.get(i) ?? []).map((w) => (w.reason === "busy" ? { ...w, by: holdersOf(w.resource) } : w)),
  }));
  const names = new Set(Object.keys(s.capacity));
  for (const r of [...s.grants, ...s.queue]) for (const n of Object.keys(r.needs)) names.add(n);
  const resources: ResourceView[] = [...names].sort().map((name) => ({
    name,
    capacity: s.capacity[name] ?? 1,
    inUse: s.grants.reduce((n, g) => n + (g.needs[name] ?? 0), 0),
    holders: holdersOf(name),
    waiters: s.queue.filter((r) => name in r.needs).map((r) => r.id),
  }));
  return { nowMs: Date.now(), resources, holders: s.grants, queue };
}

const later = <T,>(ms: number, f: () => T) =>
  new Promise<T>((resolve, reject) =>
    setTimeout(() => {
      try {
        resolve(f());
      } catch (e) {
        reject(e);
      }
    }, ms),
  );

function queuedIndex(s: MockState, id: string): number {
  const i = s.queue.findIndex((r) => r.id === id);
  if (i >= 0) return i;
  if (s.grants.some((g) => g.id === id)) throw `${id} is not queued — it already holds its resources and is running`;
  throw `no queued request ${id}`;
}

/** Answer one `resource_*` command, or `undefined` when `cmd` is not one. */
export function handleResourceIPC(cmd: string, args: Record<string, unknown>): Promise<unknown> | undefined {
  const s = st();
  switch (cmd) {
    case "resource_snapshot":
      return later(IPC_DELAY_MS, () => snapshot(s));
    case "resource_move":
      return later(IPC_DELAY_MS, () => {
        const id = String(args.id);
        const target = args.target as MoveTarget;
        const from = queuedIndex(s, id);
        const [r] = s.queue.splice(from, 1);
        let to: number;
        if ("index" in target) to = Math.min(Math.max(target.index, 1) - 1, s.queue.length);
        else {
          const at = s.queue.findIndex((q) => q.id === target.before);
          to = at < 0 ? s.queue.length : at;
        }
        s.queue.splice(to, 0, { ...r, manual: true });
        return snapshot(s);
      });
    case "resource_cancel":
      return later(IPC_DELAY_MS, () => {
        const [r] = s.queue.splice(queuedIndex(s, String(args.id)), 1);
        const now = Date.now();
        s.history.push(ev("cancelled", r, now, { waitedMs: now - r.enqueuedMs }));
        return snapshot(s);
      });
    case "resource_set_capacity":
      return later(IPC_DELAY_MS, () => {
        const capacity = Number(args.capacity);
        if (!(capacity >= 1)) throw "capacity must be at least 1";
        const name = String(args.name);
        s.history.push({ tsMs: Date.now(), kind: "capacity_set", resource: name, capacity, prevCapacity: s.capacity[name], needs: {} });
        s.capacity[name] = capacity;
        return snapshot(s);
      });
    case "resource_kick":
      return later(KICK_DELAY_MS, () => {
        const id = String(args.id);
        if (s.queue.some((r) => r.id === id)) throw `${id} is still queued and holds nothing — cancel it instead`;
        const g = s.grants.find((x) => x.id === id);
        if (!g) throw `no request ${id} holds a resource`;
        const now = Date.now();
        s.history.push(ev("kicked", g, now, { heldMs: now - (g.grantedMs ?? g.enqueuedMs), exitCode: 143 }));
        s.grants = s.grants.filter((x) => x.id !== id);
        return snapshot(s);
      });
    case "resource_remove":
      return later(IPC_DELAY_MS, () => {
        const name = String(args.name);
        const using = [...s.grants, ...s.queue].filter((r) => name in r.needs).map((r) => r.id);
        if (using.length) throw `${name} is in use by ${using.join(", ")} — only an idle resource can be removed`;
        if (!(name in s.capacity)) throw `no resource ${name}`;
        s.history.push({ tsMs: Date.now(), kind: "resource_removed", resource: name, prevCapacity: s.capacity[name], needs: {} });
        delete s.capacity[name];
        return snapshot(s);
      });
    case "resource_history":
      return later(IPC_DELAY_MS, () => readHistory(s, (args.query ?? {}) as HistoryQuery));
    default:
      return undefined;
  }
}

/** Ids of the `fleet__job run` calls in the mock transcript (`data.ts`) — one
 *  per state the job card draws. QUEUED / HOLDING are requests seeded above. */
export const MOCK_JOB_IDS = {
  queued: "p1a1fc40ef29",
  holding: "p1a1fc3d8e44",
  exited: "p1a1fb07c2d5",
  failed: "p1a1fb07c9e1",
  gone: "p1a1f9000000",
} as const;

/**
 * `get_workspace_proc` for the job card: the record of a job the queue no
 * longer places. Exited ones answer; anything else is "no such proc", which is
 * what a cleared record or another machine's job looks like for real.
 */
export function mockJobRecord(id: string): Promise<ProcRecord> {
  const now = Date.now();
  const base = (extra: Partial<ProcRecord>): ProcRecord => ({
    id,
    workspacePath: "/Users/demo/workspace/claude-fleet",
    command: "",
    status: "exited",
    startedMs: now,
    cols: 120,
    rows: 40,
    ...extra,
  });
  return later(IPC_DELAY_MS, () => {
    if (id === MOCK_JOB_IDS.exited) {
      return base({
        command: "cargo test -p claw-fleet-core --test resource_queue_e2e",
        exitCode: 0,
        startedMs: now - 26 * 60_000,
        finishedMs: now - 12 * 60_000,
        queue: { needs: { cargo: 1 }, priority: "normal", grantedMs: now - 24 * 60_000 - 35_000 },
      });
    }
    if (id === MOCK_JOB_IDS.failed) {
      return base({ command: "pnpm build", exitCode: 1, startedMs: now - 9 * 60_000, finishedMs: now - 8 * 60_000 - 17_000 });
    }
    throw `no proc ${id}`;
  });
}
