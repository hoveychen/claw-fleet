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
  return {
    capacity: { gpu: 1, "staging-db": 1, cargo: 2 },
    holders: [train, e2e],
    queue: [
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
    pid: r.holder.pid,
    command: r.command || undefined,
    workspace: r.workspace,
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
