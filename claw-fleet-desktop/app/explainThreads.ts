import type { ExplainRecord } from "./explainApi";

/**
 * One side question and the follow-ups that continue it, oldest first.
 *
 * `id` is the root record's id, which is also the group's React key: every
 * follow-up carries the whole chain in `thread` (oldest first), so the root is
 * `thread[0] ?? id` for any record in the chain.
 */
export type ExplainThread = {
  id: string;
  records: ExplainRecord[];
};

/** The record that started `rec`'s chain — itself, if it started one. */
export function threadRootId(rec: ExplainRecord): string {
  return rec.thread?.[0] ?? rec.id;
}

/**
 * Group side questions into chains for display: chains newest-first (a reader
 * cares about what was just asked), records *within* a chain oldest-first (a
 * follow-up only reads as an answer to the turn above it).
 *
 * A chain's position is decided by its newest record, not its root — asking a
 * follow-up should pull that conversation back to the top rather than leave it
 * buried under questions asked before it.
 */
export function groupExplainThreads(answers: ExplainRecord[]): ExplainThread[] {
  const byRoot = new Map<string, ExplainRecord[]>();
  for (const rec of answers) {
    const root = threadRootId(rec);
    const bucket = byRoot.get(root);
    if (bucket) bucket.push(rec);
    else byRoot.set(root, [rec]);
  }
  const threads: ExplainThread[] = [];
  for (const [id, records] of byRoot) {
    records.sort((a, b) => a.createdMs - b.createdMs);
    threads.push({ id, records });
  }
  const newest = (th: ExplainThread) =>
    th.records.reduce((max, r) => Math.max(max, r.createdMs), 0);
  threads.sort((a, b) => newest(b) - newest(a));
  return threads;
}
