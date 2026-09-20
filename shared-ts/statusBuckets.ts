/**
 * The run-status sections the task list offers as an alternative to grouping by
 * repository. Shared by the desktop task rail and the mobile task page so both
 * cut the same list into the same five headings in the same order.
 *
 * The buckets are the states each client's row dot already distinguishes
 * (desktop `rowBarColor`, mobile `statusTone`), so a heading never contradicts
 * the colour beside it. Note this is the *run* status, not the manual review
 * mark — that dimension is already served by the pending/done filter segments.
 */
export type StatusBucket =
  | "running"
  | "waitingInput"
  | "error"
  | "watching"
  | "ended";

/** Section order: the states that want a human first, ended work last. */
export const STATUS_BUCKETS: StatusBucket[] = [
  "running",
  "waitingInput",
  "error",
  "watching",
  "ended",
];

/** Statuses that mean the agent is wedged or cut off, not working — they keep a
 *  live process and write nothing, so without this branch they would decay into
 *  "ended" and hide the one state that always needs a human. */
const ERROR_STATUSES = new Set([
  "rateLimited",
  "serverErrored",
  "remoteDisconnected",
  "stuck",
]);

/** Statuses that mean a turn is genuinely in flight. */
const WORKING_STATUSES = new Set([
  "thinking",
  "executing",
  "streaming",
  "processing",
  "active",
  "delegating",
]);

/**
 * Which section a session belongs to.
 *
 * `quiet` is the caller's latched "process alive but the transcript has gone
 * quiet" verdict (desktop `isQuietAliveSticky`, mobile `statusTone`'s latch).
 * It is passed in rather than recomputed here because each client keys its
 * hysteresis differently — mobile has to scope the key by device.
 */
export function statusBucketOf(
  status: string,
  opts: { procAlive: boolean; quiet: boolean },
): StatusBucket {
  // Checked ahead of everything: a watch-parked session has no process and
  // writes nothing, so every later branch would read it as ended even though a
  // Fleet timer is going to bring it back.
  if (status === "watching") return "watching";
  if (status === "waitingInput") return "waitingInput";
  if (ERROR_STATUSES.has(status)) return "error";
  if (WORKING_STATUSES.has(status) || opts.quiet || opts.procAlive) {
    return "running";
  }
  return "ended";
}

/**
 * Which section a *collapsed relay chain* belongs to, given its members' own
 * buckets: the most salient one, in `STATUS_BUCKETS` order (a running hop wins
 * over a waiting one, anything live wins over ended).
 *
 * A chain is one unit of work, so bucketing its hops individually split it
 * across two headings: every retired hop reads as `ended` the moment it hands
 * off (no process, status decayed), so a chain that is very much alive showed a
 * collapsed "ended" row for its retired hops alongside the running tip. Judge
 * the chain by its liveliest member instead, the same way `chainBarColor` /
 * `chainTone` already pick the collapsed row's dot.
 *
 * An empty member list cannot happen through the render path (a group always
 * has ≥2 members) and falls back to `ended`.
 */
export function chainBucketOf(members: StatusBucket[]): StatusBucket {
  let best = STATUS_BUCKETS.length;
  for (const b of members) {
    const rank = STATUS_BUCKETS.indexOf(b);
    if (rank >= 0 && rank < best) best = rank;
  }
  return STATUS_BUCKETS[best] ?? "ended";
}
