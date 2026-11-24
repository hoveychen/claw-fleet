// Pure view logic for the resource-queue page (ResourcesView): grouping the
// snapshot per resource, the move targets behind the up/down/top buttons, and
// the "waiting on X held by Y" lines, and how a session lease (no command,
// held across turns) reads in a row. Kept out of the component so it can be
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
  ResourceEvent,
  ResourceEventKind,
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

/** One-line command for a row; an incognito session's job has none. A session
 *  lease runs no command: it reads as "lease · <what the session said>". */
export function commandLabel(r: ResourceRequest, max = 80): string {
  if (r.kind === "lease") {
    const note = (r.note ?? "").split(/\s+/).filter(Boolean).join(" ");
    return clip(t("租约 · {0}", note || t("没写用途")), max);
  }
  const line = r.command.split(/\s+/).filter(Boolean).join(" ");
  if (!line) return t("（隐身任务，命令不显示）");
  return clip(line, max);
}

function clip(s: string, max: number): string {
  return s.length > max ? `${s.slice(0, max - 1)}…` : s;
}

/** HH:MM today, MM-DD HH:MM on another day. */
export function clockTime(tsMs: number, now = Date.now()): string {
  const d = new Date(tsMs);
  const pad = (n: number) => String(n).padStart(2, "0");
  const hm = `${pad(d.getHours())}:${pad(d.getMinutes())}`;
  return d.toDateString() === new Date(now).toDateString() ? hm : `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${hm}`;
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
  const lines = q.waitOn
    .filter((w) => w.reason !== "after")
    .map((w: WaitOn) =>
      w.reason === "busy"
        ? t("等 {0}，被 {1} 占用", w.resource, who(w.by))
        : t("{0} 已留给排在前面的 {1}", w.resource, who(w.by)),
    );
  // `after` comes once per resource; one line says it for all of them.
  if (q.waitOn.some((w) => w.reason === "after") && q.request.afterMs != null) {
    lines.unshift(t("{0} 之后才会拿到，之前不占位", clockTime(q.request.afterMs, snap.nowMs)));
  }
  return lines;
}

// ── History (`resource_history`) ────────────────────────────────────────────

export type EventTone = "neutral" | "accent" | "ok" | "warn" | "bad";

const KIND: Record<ResourceEventKind, [string, EventTone]> = {
  enqueued: ["入队", "neutral"],
  granted: ["拿到", "accent"],
  released: ["释放", "ok"],
  kicked: ["被踢", "bad"],
  cancelled: ["已取消", "warn"],
  timed_out: ["等待超时", "warn"],
  aborted: ["放弃排队", "warn"],
  reaped: ["已回收", "bad"],
  transferred: ["转交", "accent"],
  capacity_set: ["容量变更", "neutral"],
  resource_removed: ["资源删除", "neutral"],
};

export function eventKind(kind: ResourceEventKind): { label: string; tone: EventTone } {
  const [zh, tone] = KIND[kind] ?? [String(kind), "neutral"];
  return { label: t(zh), tone };
}

/** A stable key for one event (several can share a millisecond). */
export function eventKey(e: ResourceEvent): string {
  return `${e.tsMs}:${e.kind}:${e.requestId ?? e.resource ?? ""}`;
}

/** The event's subject line: the command, or the resource for table changes. */
export function eventTitle(e: ResourceEvent): string {
  if (e.resource) {
    if (e.capacity != null && e.prevCapacity != null) return t("{0} 容量 {1} → {2}", e.resource, e.prevCapacity, e.capacity);
    if (e.capacity != null) return t("{0} 容量 {1}", e.resource, e.capacity);
    return e.resource;
  }
  if (e.requestKind === "lease") {
    const note = (e.note ?? "").split(/\s+/).filter(Boolean).join(" ");
    return clip(note ? t("租约 · {0}", note) : t("租约"), 80);
  }
  const line = (e.command ?? "").split(/\s+/).filter(Boolean).join(" ");
  if (!line) return e.requestId ?? "";
  return clip(line, 80);
}

/** "gpu, board:2" — a needs map as the CLI prints it. */
export function fmtNeeds(needs: Record<string, number | undefined> | null | undefined): string {
  return Object.entries(needs ?? {})
    .map(([n, u]) => (u === 1 ? n : `${n}:${u}`))
    .join(", ");
}

/** What the session behind a queued lease piece already holds (other lease
 *  pieces of the same session): the piece waits for more on top of that. */
export function sessionHolds(r: ResourceRequest, holders: ResourceRequest[]): string {
  if (r.kind !== "lease" || !r.sessionId) return "";
  const held: Record<string, number> = {};
  for (const h of holders) {
    if (h.kind !== "lease" || h.sessionId !== r.sessionId || h.id === r.id) continue;
    for (const [n, u] of Object.entries(h.needs)) held[n] = (held[n] ?? 0) + (u ?? 0);
  }
  return fmtNeeds(held);
}

/** "gpu, db · 等了 12s · 持有 3m · 退出码 1". */
export function eventDetails(e: ResourceEvent): string {
  const needs = fmtNeeds(e.needs);
  const kept = fmtNeeds(e.kept);
  const sid = (s: string | null | undefined) => (s ?? "").slice(0, 8);
  return [
    needs || null,
    kept ? t("仍持有 {0}", kept) : null,
    e.kind === "transferred" && e.fromSessionId && e.sessionId
      ? t("会话 {0} → {1}", sid(e.fromSessionId), sid(e.sessionId))
      : e.requestKind === "lease" && e.sessionId
        ? t("会话 {0}", sid(e.sessionId))
        : null,
    e.waitedMs != null && e.kind !== "enqueued" ? t("等了 {0}", fmtDuration(e.waitedMs)) : null,
    e.heldMs != null ? t("持有 {0}", fmtDuration(e.heldMs)) : null,
    e.exitCode != null ? t("退出码 {0}", e.exitCode) : null,
  ]
    .filter(Boolean)
    .join(" · ");
}

/** HH:MM:SS today, MM-DD HH:MM otherwise. */
export function eventTime(tsMs: number, now = Date.now()): string {
  const d = new Date(tsMs);
  const pad = (n: number) => String(n).padStart(2, "0");
  if (d.toDateString() === new Date(now).toDateString()) {
    return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
  }
  return `${pad(d.getMonth() + 1)}-${pad(d.getDate())} ${pad(d.getHours())}:${pad(d.getMinutes())}`;
}
