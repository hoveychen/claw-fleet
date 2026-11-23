// `?mock` fixture for the resource-queue page. Stateful, so the reorder and
// cancel buttons visibly do something in the mock. `?mock&resources=empty`
// starts with no resources (the empty state).
//
// Move/cancel mirror `resource_queue.rs::State::{move_request, cancel}`; the
// `waitOn` reasons are kept as authored rather than recomputed — enough for a
// UI fixture, not a second scheduler.

import type {
  HistoryQuery,
  MoveTarget,
  ResourceEvent,
  ResourceEventKind,
  ResourceHistory,
  ProcRecord,
  QueuedRequest,
  ResourceRequest,
  ResourceSnapshot,
} from "../generated/types";

const MIN = 60_000;

function req(
  id: string,
  needs: Record<string, number>,
  command: string,
  workspace: string,
  ageMs: number,
  extra: Partial<ResourceRequest> = {},
): ResourceRequest {
  return {
    id,
    kind: "job",
    needs,
    priority: "normal",
    enqueuedMs: Date.now() - ageMs,
    manual: false,
    holder: { pid: 40000 + id.length },
    command,
    workspace,
    ...extra,
  };
}

/** A session lease (`fleet resource take` / `queue`): no command, no process. */
const LEASE: Partial<ResourceRequest> = { kind: "lease", holder: { pid: 0 } };

interface MockState {
  capacity: Record<string, number>;
  holders: ResourceRequest[];
  queue: { request: ResourceRequest; waitOn: QueuedRequest["waitOn"] }[];
}

function initial(): MockState {
  const now = Date.now();
  const train = req("job-7f3a", { gpu: 1 }, "python train.py --epochs 40 --batch 64", "/Users/boss/workspace/vision-lab", 52 * MIN, {
    grantedMs: now - 47 * MIN,
  });
  const e2e = req("job-91c2", { "staging-db": 1 }, "pnpm test:e2e --project=chromium", "/Users/boss/workspace/shop-web", 6 * MIN, {
    grantedMs: now - 5 * MIN,
  });
  const board = req("l-4b7e", { "esp32-board": 1 }, "", "/Users/boss/workspace/esp-os", 24 * MIN, {
    ...LEASE,
    grantedMs: now - 22 * MIN,
    sessionId: "5d2e8f14-7a3c-4b09-9e61-2f8c0d4b7a19",
    note: "flash firmware and run the chat e2e on the board",
    idleSinceMs: now - 40_000,
  });
  return {
    capacity: { gpu: 1, "staging-db": 1, cargo: 2, "esp32-board": 1 },
    holders: [train, e2e, board],
    queue: [
      {
        request: req("l-9c21", { "esp32-board": 1 }, "", "/Users/boss/workspace/esp-os", 4 * MIN, {
          ...LEASE,
          sessionId: "4e9c2a71-d6b3-4085-a1f7-93c5e0b8d24f",
          note: "overnight battery soak",
          afterMs: now + 35 * MIN,
        }),
        waitOn: [{ resource: "esp32-board", reason: "after", by: [] }],
      },
      {
        request: req("job-a810", { gpu: 1 }, "python eval.py --ckpt runs/latest", "/Users/boss/workspace/vision-lab", 18 * MIN, {
          priority: "high",
        }),
        waitOn: [{ resource: "gpu", reason: "busy", by: [train.id] }],
      },
      {
        request: req("job-b442", { gpu: 1, "staging-db": 1 }, "python backfill_embeddings.py --since 2026-11-01", "/Users/boss/workspace/search-api", 11 * MIN),
        waitOn: [
          { resource: "gpu", reason: "busy", by: [train.id] },
          { resource: "staging-db", reason: "busy", by: [e2e.id] },
        ],
      },
      {
        request: req("job-c019", { "staging-db": 1 }, "pnpm db:migrate && pnpm test:integration", "/Users/boss/workspace/shop-web", 3 * MIN),
        waitOn: [{ resource: "staging-db", reason: "reserved", by: ["job-b442"] }],
      },
      {
        request: req("job-d5e7", { gpu: 2 }, "", "/Users/boss/workspace/vision-lab", 2 * MIN),
        waitOn: [{ resource: "gpu", reason: "busy", by: [train.id] }],
      },
    ],
  };
}

const empty = new URLSearchParams(window.location.search).get("resources") === "empty";
let state: MockState = empty ? { capacity: {}, holders: [], queue: [] } : initial();
// The fixture asks for 2 gpu units, so give gpu enough capacity to be valid.
if (!empty) state.capacity.gpu = 2;

export function mockResourceSnapshot(): ResourceSnapshot {
  const names = Object.keys(state.capacity).sort();
  return {
    nowMs: Date.now(),
    resources: names.map((name) => ({
      name,
      capacity: state.capacity[name],
      inUse: state.holders.reduce((n, h) => n + (h.needs[name] ?? 0), 0),
      holders: state.holders.filter((h) => name in h.needs).map((h) => h.id),
      waiters: state.queue.filter((q) => name in q.request.needs).map((q) => q.request.id),
    })),
    holders: state.holders,
    queue: state.queue.map((q, i) => ({ position: i + 1, request: q.request, waitOn: q.waitOn })),
  };
}

function queuedIndex(id: string): number {
  const i = state.queue.findIndex((q) => q.request.id === id);
  if (i < 0) throw new Error(`no queued request ${id}`);
  return i;
}

export function mockResourceMove(id: string, target: MoveTarget): ResourceSnapshot {
  const from = queuedIndex(id);
  const queue = [...state.queue];
  const [item] = queue.splice(from, 1);
  const to =
    "index" in target
      ? Math.min(Math.max(target.index, 1) - 1, queue.length)
      : queue.findIndex((q) => q.request.id === target.before);
  if (to < 0) throw new Error(`no queued request ${"before" in target ? target.before : ""}`);
  queue.splice(to, 0, { ...item, request: { ...item.request, manual: true } });
  state = { ...state, queue };
  return mockResourceSnapshot();
}

export function mockResourceCancel(id: string): ResourceSnapshot {
  const from = queuedIndex(id);
  const r = state.queue[from].request;
  history.push(ev("cancelled", r, Date.now(), { waitedMs: Date.now() - r.enqueuedMs }));
  state = { ...state, queue: state.queue.filter((_, i) => i !== from) };
  return mockResourceSnapshot();
}

/** `resource_lease_release`: a lease held or queued, released on its
 *  session's behalf. Anything else is refused, as core does. */
export function mockResourceLeaseRelease(id: string): ResourceSnapshot {
  const now = Date.now();
  const held = state.holders.find((h) => h.id === id && h.kind === "lease");
  if (held) {
    history.push(ev("kicked", held, now, { pid: undefined, heldMs: now - (held.grantedMs ?? held.enqueuedMs) }));
    state = { ...state, holders: state.holders.filter((h) => h.id !== id) };
    return mockResourceSnapshot();
  }
  const queued = state.queue.find((q) => q.request.id === id && q.request.kind === "lease");
  if (!queued) throw new Error(`no lease ${id}`);
  history.push(ev("cancelled", queued.request, now, { pid: undefined, waitedMs: now - queued.request.enqueuedMs }));
  state = { ...state, queue: state.queue.filter((q) => q.request.id !== id) };
  return mockResourceSnapshot();
}

// ── History ──────────────────────────────────────────────────────────────────
// An hour of finished work plus the live table's own enqueue/grant events,
// so the history card has every kind of row to show.

function ev(kind: ResourceEventKind, r: ResourceRequest, tsMs: number, extra: Partial<ResourceEvent> = {}): ResourceEvent {
  return {
    tsMs,
    kind,
    requestId: r.id,
    requestKind: r.kind,
    needs: r.needs,
    priority: r.priority,
    pid: r.kind === "lease" ? undefined : r.holder.pid,
    sessionId: r.sessionId,
    command: r.command || undefined,
    workspace: r.workspace,
    note: r.note,
    ...extra,
  };
}

function seedHistory(): ResourceEvent[] {
  if (empty) return [];
  const now = Date.now();
  const out: ResourceEvent[] = [
    { tsMs: now - 70 * MIN, kind: "capacity_set", resource: "gpu", capacity: 2, prevCapacity: 1, needs: {} },
  ];
  const done = (r: ResourceRequest, waitMs: number, holdMs: number, end: ResourceEventKind, exitCode?: number) => {
    out.push(ev("enqueued", r, r.enqueuedMs));
    if (end === "cancelled" || end === "timed_out") {
      out.push(ev(end, r, r.enqueuedMs + waitMs, { waitedMs: waitMs }));
      return;
    }
    out.push(ev("granted", r, r.enqueuedMs + waitMs, { waitedMs: waitMs }));
    out.push(ev(end, r, r.enqueuedMs + waitMs + holdMs, { heldMs: holdMs, exitCode }));
  };
  const ws = "/Users/boss/workspace/vision-lab";
  done(req("job-5a01", { gpu: 1 }, "python train.py --epochs 10", ws, 66 * MIN), 0, 9 * MIN, "released", 0);
  done(req("job-5b17", { "staging-db": 1 }, "pnpm test:e2e --project=webkit", "/Users/boss/workspace/shop-web", 60 * MIN), 30_000, 4 * MIN, "released", 1);
  done(req("job-5c4e", { gpu: 1 }, "python sweep.py --trials 50", ws, 57 * MIN), 2 * MIN, 12 * MIN, "kicked", 143);
  done(req("job-5d90", { "staging-db": 1 }, "pnpm db:seed", "/Users/boss/workspace/shop-web", 40 * MIN), 5 * MIN, 0, "timed_out");
  done(req("job-5e22", { gpu: 1 }, "python infer.py --batch 8", ws, 35 * MIN), 0, 3 * MIN, "reaped");
  done(req("job-5f63", { cargo: 1 }, "cargo build --release", "/Users/boss/workspace/search-api", 30 * MIN), 20_000, 0, "cancelled");
  // A lease carried to the successor at a handoff, then released by it; and
  // one whose session ended without letting go.
  const from = "7c41d9e2-b03a-4e5f-8d17-3a9e2c6b0f58";
  const to = "1f8a6c3d-92e4-4b70-a5d1-c0e7b4f29a63";
  const handed = req("l-3f10", { "esp32-board": 1 }, "", "/Users/boss/workspace/esp-os", 62 * MIN, {
    ...LEASE,
    sessionId: from,
    note: "flash v0.9 firmware and run the wake-word e2e",
  });
  const leaseEv = (kind: ResourceEventKind, tsMs: number, extra: Partial<ResourceEvent> = {}) =>
    ev(kind, handed, tsMs, { pid: undefined, sessionId: from, ...extra });
  out.push(leaseEv("enqueued", handed.enqueuedMs), leaseEv("granted", handed.enqueuedMs, { waitedMs: 0 }));
  out.push(leaseEv("transferred", handed.enqueuedMs + 9 * MIN, { sessionId: to, fromSessionId: from }));
  out.push(leaseEv("released", handed.enqueuedMs + 16 * MIN, { sessionId: to, heldMs: 16 * MIN }));
  const lapsed = req("l-6a02", { "esp32-board": 1 }, "", "/Users/boss/workspace/esp-os", 44 * MIN, {
    ...LEASE,
    sessionId: "e93b1c07-4d2a-4f86-b5e0-8a1c6d2f9b34",
    note: "bisect the audio driver regression",
  });
  done(lapsed, 0, 14 * MIN, "reaped");
  for (const h of state.holders) {
    out.push(ev("enqueued", h, h.enqueuedMs));
    out.push(ev("granted", h, h.grantedMs ?? h.enqueuedMs, { waitedMs: (h.grantedMs ?? h.enqueuedMs) - h.enqueuedMs }));
  }
  for (const q of state.queue) out.push(ev("enqueued", q.request, q.request.enqueuedMs));
  return out;
}

const history: ResourceEvent[] = seedHistory();

export function mockResourceHistory(q: HistoryQuery): ResourceHistory {
  const limit = Math.min(Math.max(q.limit ?? 200, 1), 2000);
  const all = [...history]
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

/** `proc_get` for the job card: records of jobs the queue no longer places.
 *  Anything else is "no such proc" — the card then shows a bare "ended". */
export function mockJobRecord(id: string): ProcRecord {
  const now = Date.now();
  const base = { id, workspacePath: "/Users/boss/workspace/api-server", status: "exited" as const, cols: 120, rows: 40 };
  if (id === "job-e3a1") {
    return { ...base, command: "cargo test --workspace", exitCode: 0, startedMs: now - 31 * MIN, finishedMs: now - 14 * MIN,
      queue: { needs: { cargo: 1 }, priority: "normal", grantedMs: now - 29 * MIN } };
  }
  if (id === "job-e3a2") {
    return { ...base, command: "pnpm build", exitCode: 1, startedMs: now - 9 * MIN, finishedMs: now - 8 * MIN - 17_000 };
  }
  throw new Error(`no proc ${id}`);
}
