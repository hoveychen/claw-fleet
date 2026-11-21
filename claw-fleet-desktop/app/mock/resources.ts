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
 * `?mock&resources=empty` starts with an empty table.
 */
import type {
  MoveTarget,
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
  if (empty) return { capacity: {}, grants: [], queue: [] };
  const now = Date.now();
  return {
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
  for (const r of taken) s.grants.push({ ...r, grantedMs: now });
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
        s.queue.splice(queuedIndex(s, String(args.id)), 1);
        return snapshot(s);
      });
    case "resource_set_capacity":
      return later(IPC_DELAY_MS, () => {
        const capacity = Number(args.capacity);
        if (!(capacity >= 1)) throw "capacity must be at least 1";
        s.capacity[String(args.name)] = capacity;
        return snapshot(s);
      });
    case "resource_kick":
      return later(KICK_DELAY_MS, () => {
        const id = String(args.id);
        if (s.queue.some((r) => r.id === id)) throw `${id} is still queued and holds nothing — cancel it instead`;
        if (!s.grants.some((g) => g.id === id)) throw `no request ${id} holds a resource`;
        s.grants = s.grants.filter((g) => g.id !== id);
        return snapshot(s);
      });
    case "resource_remove":
      return later(IPC_DELAY_MS, () => {
        const name = String(args.name);
        const using = [...s.grants, ...s.queue].filter((r) => name in r.needs).map((r) => r.id);
        if (using.length) throw `${name} is in use by ${using.join(", ")} — only an idle resource can be removed`;
        if (!(name in s.capacity)) throw `no resource ${name}`;
        delete s.capacity[name];
        return snapshot(s);
      });
    default:
      return undefined;
  }
}
