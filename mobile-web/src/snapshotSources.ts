// Which agent answered each pending_snapshot — a diagnostic ledger kept on the phone.
//
// The relay broadcasts each client request to **all** agents in the channel (Channel
// .agents is a map), and the phone trusts the first reply. So a second agent appearing
// anywhere outside the desktop can answer for it; if it can't see ~/.fleet on this
// machine (e.g., running under a redirected FLEET_HOME), it returns an empty list, and
// the snapshot is an authoritative full table replacement on the client — cards vanish.
// decisionReconcile.ts intercepts these empty snapshots on the spot; this keeps a
// record of "who showed up" so the next time it repeats, we can see the impostor's
// host/pid/home at a glance in the "More" page, without digging server logs.
import type { AgentFingerprint } from "./types";

/** How many sources the ledger keeps at most. Normally just 1 (desktop), extras are
 *  what to investigate. */
export const MAX_SNAPSHOT_SOURCES = 8;

/** How many PIDs per source. Enough to see "it restarted", no need for full history. */
export const MAX_SOURCE_PIDS = 8;

export interface SnapshotSource {
  /** Result of agentKeyOf(); `undefined` means the other side had no fingerprint
   *  (old desktop). */
  key?: string;
  /** The fingerprint of the process that most recently returned a snapshot (pid
   *  updated to latest). */
  agent?: AgentFingerprint;
  /** PIDs this source has used, in order of first appearance. Length > 1 means it
   *  restarted. */
  pids: number[];
  firstAt: number;
  lastAt: number;
  /** Total snapshots returned by this source. */
  snapshots: number;
  /** How many empty snapshots were dropped because this source looked suspicious. */
  ignored: number;
  /** Was the most recent one trusted as the primary agent (real source of cards). */
  trusted: boolean;
}

export interface SnapshotSourceEvent {
  key?: string;
  agent?: AgentFingerprint;
  at: number;
  trusted: boolean;
  ignored: boolean;
}

/** Append an unseen PID, preserving order of first appearance; when full, drop the
 *  oldest. */
function withPid(pids: number[], pid: number | undefined): number[] {
  if (pid === undefined || pids.includes(pid)) return pids;
  const next = [...pids, pid];
  return next.length > MAX_SOURCE_PIDS ? next.slice(next.length - MAX_SOURCE_PIDS) : next;
}

/** Pure function: merge a snapshot arrival into the ledger, return new array (don't
 *  mutate input). */
export function recordSnapshotSource(
  sources: SnapshotSource[],
  ev: SnapshotSourceEvent,
): SnapshotSource[] {
  const next = sources.map((s) =>
    s.key === ev.key
      ? {
          ...s,
          agent: ev.agent ?? s.agent,
          pids: withPid(s.pids, ev.agent?.pid),
          lastAt: ev.at,
          snapshots: s.snapshots + 1,
          ignored: s.ignored + (ev.ignored ? 1 : 0),
          trusted: ev.trusted,
        }
      : s,
  );
  if (!next.some((s) => s.key === ev.key)) {
    next.push({
      key: ev.key,
      agent: ev.agent,
      pids: withPid([], ev.agent?.pid),
      firstAt: ev.at,
      lastAt: ev.at,
      snapshots: 1,
      ignored: ev.ignored ? 1 : 0,
      trusted: ev.trusted,
    });
  }
  // When over limit, drop the one that's been silent longest — active sources always
  // stick around.
  if (next.length > MAX_SNAPSHOT_SOURCES) {
    next.sort((a, b) => b.lastAt - a.lastAt);
    return next.slice(0, MAX_SNAPSHOT_SOURCES);
  }
  return next;
}
