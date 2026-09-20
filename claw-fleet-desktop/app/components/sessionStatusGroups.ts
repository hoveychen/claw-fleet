import type { SessionInfo } from "../types";
import { isQuietAliveSticky } from "../types";
import {
  STATUS_BUCKETS,
  chainBucketOf,
  statusBucketOf as bucketOf,
  type StatusBucket,
} from "../../../shared-ts/statusBuckets";
import type { RenderItem } from "./sessionGroups";

export { STATUS_BUCKETS, type StatusBucket };

/** One run-status section of already-folded render items. */
export interface StatusItemGroup {
  bucket: StatusBucket;
  /** Synthetic key for the shared collapse bookkeeping, which is keyed by the
   *  section's `path`. Prefixed so it can never collide with a real workspace
   *  path in the same persisted list. */
  path: string;
  items: RenderItem[];
}

/** The collapse key a status section is folded under. */
export function statusSectionPath(bucket: StatusBucket): string {
  return `status:${bucket}`;
}

function activityMs(session: SessionInfo): number {
  return session.agentLastActivityMs ?? session.lastActivityMs;
}

/**
 * Which bucket a session falls in. The latch is consulted so a session parked
 * on one long tool call — process alive, transcript gone quiet, status decayed
 * to idle — lands under "running" rather than "ended", matching the faded-green
 * dot the row wears.
 */
export function statusBucketOf(s: SessionInfo): StatusBucket {
  return bucketOf(s.status, {
    procAlive: s.procAlive,
    quiet: isQuietAliveSticky(s),
  });
}

/** Which bucket a rendered row falls in. A collapsed relay chain is one unit of
 *  work, so it is judged by its liveliest hop rather than hop by hop — see
 *  `chainBucketOf`. */
export function itemBucketOf(item: RenderItem): StatusBucket {
  if (item.kind === "single") return statusBucketOf(item.session);
  return chainBucketOf(item.members.map(statusBucketOf));
}

/** The activity a rendered row sorts on: for a chain, its most recent hop, the
 *  same value the rail's flat sort floats the chain up by. */
function itemActivityMs(item: RenderItem): number {
  if (item.kind === "single") return activityMs(item.session);
  return item.members.reduce((max, m) => Math.max(max, activityMs(m)), 0);
}

/**
 * Turn the task rail's already-folded render items into run-status sections, in
 * the fixed `STATUS_BUCKETS` order. Empty buckets are dropped — an
 * always-present "0 running" heading is noise, and the reader can tell a state
 * is empty by its absence.
 *
 * Folding happens *before* the split (unlike the repository grouping, which
 * folds inside each section) because a relay chain must not be cut in half by
 * run status: hop by hop, every retired hop reads as `ended` the instant it
 * hands off, so a live chain used to show a collapsed "ended" row beside its
 * own running tip.
 *
 * `preserveOrder` has the same meaning as in `groupSessionsByWorkspace`: the
 * rail passes it while its sort freeze is engaged, and it skips the
 * within-section re-sort that would otherwise undo the freeze. Section order is
 * fixed either way, so it cannot slide under the cursor.
 */
export function groupItemsByStatus(
  items: RenderItem[],
  { preserveOrder = false }: { preserveOrder?: boolean } = {},
): StatusItemGroup[] {
  const byBucket = new Map<StatusBucket, RenderItem[]>();
  for (const item of items) {
    const bucket = itemBucketOf(item);
    const arr = byBucket.get(bucket);
    if (arr) arr.push(item);
    else byBucket.set(bucket, [item]);
  }

  const groups: StatusItemGroup[] = [];
  for (const bucket of STATUS_BUCKETS) {
    const members = byBucket.get(bucket);
    if (!members || members.length === 0) continue;
    if (!preserveOrder) {
      members.sort((a, b) => itemActivityMs(b) - itemActivityMs(a));
    }
    groups.push({ bucket, path: statusSectionPath(bucket), items: members });
  }
  return groups;
}
