import type { SessionInfo } from "../types";
import { LIVE_STATUSES, isQuietAliveSticky } from "../types";

/**
 * The buckets the task rail groups by when the reader picks "by status".
 *
 * They are deliberately the four states the row's own run dot already
 * distinguishes (see `rowBarColor`): green = running, amber = parked for input,
 * violet = parked on a `fleet watch`, no dot = ended. Grouping by anything else
 * would put a row under a heading that contradicts the colour next to it.
 *
 * Note this is the *run* status, not the manual review mark — that dimension is
 * already served by the pending/done filter segments above the list.
 */
export type StatusBucket = "running" | "waitingInput" | "watching" | "ended";

/** Section order: the states that want attention first, ended work last. */
export const STATUS_BUCKETS: StatusBucket[] = [
  "running",
  "waitingInput",
  "watching",
  "ended",
];

export interface StatusSessionGroup {
  bucket: StatusBucket;
  /** Synthetic key for the shared collapse bookkeeping, which is keyed by the
   *  section's `path`. Prefixed so it can never collide with a real workspace
   *  path in the same persisted list. */
  path: string;
  sessions: SessionInfo[];
}

/** The collapse key a status section is folded under. */
export function statusSectionPath(bucket: StatusBucket): string {
  return `status:${bucket}`;
}

function activityMs(session: SessionInfo): number {
  return session.agentLastActivityMs ?? session.lastActivityMs;
}

/**
 * Which bucket a session falls in. `isQuietAliveSticky` is consulted so a
 * session parked on one long tool call — process alive, transcript gone quiet,
 * status decayed to idle — lands under "running" rather than "ended", matching
 * the faded-green dot the row wears.
 */
export function statusBucketOf(s: SessionInfo): StatusBucket {
  if (s.status === "watching") return "watching";
  if (s.status === "waitingInput") return "waitingInput";
  if (LIVE_STATUSES.has(s.status) || isQuietAliveSticky(s)) return "running";
  return "ended";
}

/**
 * Turn the task rail's flat session list into run-status sections, in the fixed
 * `STATUS_BUCKETS` order. Empty buckets are dropped — an always-present "0
 * running" heading is noise, and the reader can tell a state is empty by its
 * absence.
 *
 * `preserveOrder` has the same meaning as in `groupSessionsByWorkspace`: the
 * rail passes it while its sort freeze is engaged, and it skips the
 * within-section re-sort that would otherwise undo the freeze. Section order is
 * fixed either way, so it cannot slide under the cursor.
 */
export function groupSessionsByStatus(
  sessions: SessionInfo[],
  { preserveOrder = false }: { preserveOrder?: boolean } = {},
): StatusSessionGroup[] {
  const byBucket = new Map<StatusBucket, SessionInfo[]>();
  for (const s of sessions) {
    const bucket = statusBucketOf(s);
    const arr = byBucket.get(bucket);
    if (arr) arr.push(s);
    else byBucket.set(bucket, [s]);
  }

  const groups: StatusSessionGroup[] = [];
  for (const bucket of STATUS_BUCKETS) {
    const members = byBucket.get(bucket);
    if (!members || members.length === 0) continue;
    if (!preserveOrder) members.sort((a, b) => activityMs(b) - activityMs(a));
    groups.push({ bucket, path: statusSectionPath(bucket), sessions: members });
  }
  return groups;
}
