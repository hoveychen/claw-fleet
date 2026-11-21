// Pure view logic for the resource-queue page (ResourcesView): grouping the
// snapshot per resource, the move targets behind the up/down/top buttons, and
// the "waiting on X held by Y" lines. Kept out of the component so it can be
// unit-tested without a DOM (mobile-web has no jsdom).
//
// The backend keeps ONE global queue (`resource_queue.rs`); a resource card
// only shows the slice of it that needs that resource. Up/down therefore move
// relative to the neighbours visible on that card, while "top" puts the request
// at the head of the whole queue, ahead of everything for every resource it
// needs.

import type {
  MoveTarget,
  QueuedRequest,
  ResourceRequest,
  ResourceSnapshot,
  WaitOn,
} from "./generated/types";
import { t } from "./i18n";

export interface ResourceCard {
  name: string;
  capacity: number;
  inUse: number;
  holders: ResourceRequest[];
  /** Queued requests that need this resource, in queue order. */
  waiters: QueuedRequest[];
}

export function resourceCards(snap: ResourceSnapshot): ResourceCard[] {
  const holderById = new Map(snap.holders.map((h) => [h.id, h]));
  const queuedById = new Map(snap.queue.map((q) => [q.request.id, q]));
  return snap.resources.map((r) => ({
    name: r.name,
    capacity: r.capacity,
    inUse: r.inUse,
    holders: r.holders.map((id) => holderById.get(id)).filter((h): h is ResourceRequest => !!h),
    waiters: r.waiters
      .map((id) => queuedById.get(id))
      .filter((q): q is QueuedRequest => !!q)
      .sort((a, b) => a.position - b.position),
  }));
}

export interface MoveTargets {
  top: MoveTarget | null;
  up: MoveTarget | null;
  down: MoveTarget | null;
}

/** What each reorder button sends for waiter `index` of a card; `null` = the
 *  button is disabled (already there). */
export function moveTargets(waiters: QueuedRequest[], index: number): MoveTargets {
  const self = waiters[index];
  if (!self) return { top: null, up: null, down: null };
  const prev = waiters[index - 1];
  const next = waiters[index + 1];
  return {
    top: self.position > 1 ? { index: 1 } : null,
    up: prev ? { before: prev.request.id } : null,
    // `index` is a 1-based slot in the queue *after* this request is taken out:
    // `next` then sits at slot `next.position - 1`, so slot `next.position` is
    // directly behind it.
    down: next ? { index: next.position } : null,
  };
}

/** Compact elapsed time: 45s, 12m, 3h 5m, 2d 4h. */
export function fmtDuration(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000));
  if (s < 60) return `${s}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m`;
  const h = Math.floor(m / 60);
  if (h < 24) return m % 60 ? `${h}h ${m % 60}m` : `${h}h`;
  const d = Math.floor(h / 24);
  return h % 24 ? `${d}d ${h % 24}h` : `${d}d`;
}

/** One-line command for a row; an incognito session's job has none. */
export function commandLabel(r: ResourceRequest, max = 80): string {
  const line = r.command.split(/\s+/).filter(Boolean).join(" ");
  if (!line) return t("（隐身任务，命令不显示）");
  return line.length > max ? `${line.slice(0, max - 1)}…` : line;
}

export function workspaceName(path: string | null | undefined): string | null {
  if (!path) return null;
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** Units the request takes when it needs more than one of `resource`. */
export function unitsOf(r: ResourceRequest, resource: string): number {
  return r.needs[resource] ?? 0;
}

/** "Waiting on gpu, held by cargo test …" for each blocking resource. */
export function waitLines(q: QueuedRequest, snap: ResourceSnapshot): string[] {
  const byId = new Map<string, ResourceRequest>();
  for (const h of snap.holders) byId.set(h.id, h);
  for (const w of snap.queue) byId.set(w.request.id, w.request);
  const who = (ids: string[]) =>
    ids.map((id) => (byId.has(id) ? commandLabel(byId.get(id)!, 40) : id)).join(t("、"));
  return q.waitOn.map((w: WaitOn) =>
    w.reason === "busy"
      ? t("等 {0}，被 {1} 占用", w.resource, who(w.by))
      : t("{0} 已留给排在前面的 {1}", w.resource, who(w.by)),
  );
}
