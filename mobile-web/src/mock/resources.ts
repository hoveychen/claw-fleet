// `?mock` fixture for the resource-queue page. Stateful, so the reorder and
// cancel buttons visibly do something in the mock. `?mock&resources=empty`
// starts with no resources (the empty state).
//
// Move/cancel mirror `resource_queue.rs::State::{move_request, cancel}`; the
// `waitOn` reasons are kept as authored rather than recomputed — enough for a
// UI fixture, not a second scheduler.

import type {
  MoveTarget,
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
  state = { ...state, queue: state.queue.filter((_, i) => i !== from) };
  return mockResourceSnapshot();
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
