/**
 * Who is on screen and where: sessions grouped into one submarine pod per
 * workspace, pods shelf-packed across the sea. Pure (no canvas, no DOM) so the
 * packing is unit-tested; the view draws whatever boxes this returns.
 */

import type { SessionInfo } from "../../types";
import { isLiveMember } from "../../types";
import { LOBSTER_H, LOBSTER_W, SHRIMP_H, SHRIMP_W } from "./sprites";

/** A main session that ended this recently stays on deck, resting, so a turn
 *  that just finished does not vanish from under the user's eyes. */
export const RECENT_MS = 20 * 60_000;

export interface CrewMember {
  session: SessionInfo;
  /** Live sub-agents of this session (the shrimp). */
  subagents: SessionInfo[];
}

export interface PodIn {
  key: string;
  crew: CrewMember[];
}

/** Whether a main session belongs on screen at all. `keep` lets the caller
 *  force one in (a session with a card waiting must never be hidden). */
export function onDeck(s: SessionInfo, now: number, keep: (id: string) => boolean): boolean {
  if (s.isSubagent) return false;
  return (
    keep(s.id) ||
    isLiveMember(s) ||
    s.procAlive ||
    s.status === "watching" ||
    now - s.lastActivityMs < RECENT_MS
  );
}

/** Group the on-deck main sessions by workspace, each with its live sub-agents.
 *  Pods with something live (or a card waiting) come first, then by recency. */
export function buildRoster(
  sessions: SessionInfo[],
  now: number,
  keep: (id: string) => boolean = () => false,
): PodIn[] {
  const subs = new Map<string, SessionInfo[]>();
  for (const s of sessions) {
    if (!s.isSubagent || !s.parentSessionId || !isLiveMember(s)) continue;
    const list = subs.get(s.parentSessionId) ?? [];
    list.push(s);
    subs.set(s.parentSessionId, list);
  }
  const busy = (s: SessionInfo) => keep(s.id) || isLiveMember(s) || s.procAlive;
  const pods = new Map<string, CrewMember[]>();
  for (const s of sessions) {
    if (!onDeck(s, now, keep)) continue;
    const crew = pods.get(s.workspaceName) ?? [];
    crew.push({ session: s, subagents: subs.get(s.id) ?? [] });
    pods.set(s.workspaceName, crew);
  }
  const rank = (c: CrewMember) => (busy(c.session) ? 1 : 0);
  const out: PodIn[] = [];
  for (const [key, crew] of pods) {
    // Stable seats: by creation time, so a lobster does not hop around the pod
    // every time another one becomes active.
    crew.sort((a, b) => a.session.createdAtMs - b.session.createdAtMs);
    out.push({ key, crew });
  }
  const podRank = (p: PodIn) => Math.max(...p.crew.map(rank));
  const podRecent = (p: PodIn) => Math.max(...p.crew.map((c) => c.session.lastActivityMs));
  out.sort((a, b) => podRank(b) - podRank(a) || podRecent(b) - podRecent(a) || a.key.localeCompare(b.key));
  return out;
}

// ── Geometry (art pixels) ───────────────────────────────────────────────────

/** One seat: a lobster plus a row of shrimp under it. */
export const SLOT_W = LOBSTER_W + 6;
export const SLOT_H = LOBSTER_H + SHRIMP_H + 2;
/** Shrimp drawn per seat; the rest are a DOM count badge. */
export const MAX_SHRIMP = 2;
export const POD_PAD = 4;
/** Hull strip on top of each pod that carries the workspace name. */
export const POD_LABEL_H = 10;
const GAP = 6;
const MARGIN = 6;

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface SeatBox {
  member: CrewMember;
  /** Top-left of the lobster sprite. */
  x: number;
  y: number;
  /** Top-left of each drawn shrimp. */
  shrimp: { x: number; y: number }[];
}

export interface PodBox extends Rect {
  key: string;
  seats: SeatBox[];
  /** Crew that did not fit in this pod. */
  more: number;
}

export interface FleetLayout {
  pods: PodBox[];
  /** Pods that did not fit on screen at all. */
  hiddenPods: number;
}

/**
 * Shelf-pack pods into a `w`×`h` sea whose usable floor is `floor` (the top of
 * the sand). Each pod is as wide as its crew needs, up to the sea's width;
 * shelves are centred. Pods that do not fit are counted, never squeezed.
 */
export function layoutFleet(pods: PodIn[], w: number, floor: number): FleetLayout {
  const maxCols = Math.max(1, Math.floor((w - 2 * MARGIN - 2 * POD_PAD) / SLOT_W));
  const out: PodBox[] = [];
  let shelf: PodBox[] = [];
  let x = MARGIN;
  let y = MARGIN;
  let shelfH = 0;
  let hiddenPods = 0;

  const closeShelf = () => {
    if (!shelf.length) return;
    const used = shelf[shelf.length - 1].x + shelf[shelf.length - 1].w - MARGIN;
    const dx = Math.floor((w - 2 * MARGIN - used) / 2);
    for (const p of shelf) shift(p, Math.max(0, dx), 0);
    out.push(...shelf);
    shelf = [];
  };

  for (let i = 0; i < pods.length; i++) {
    const pod = pods[i];
    const cols = Math.min(pod.crew.length, maxCols);
    let rows = Math.ceil(pod.crew.length / cols);
    const podW = cols * SLOT_W + 2 * POD_PAD;
    if (shelf.length && x + podW > w - MARGIN) {
      closeShelf();
      y += shelfH + GAP;
      x = MARGIN;
      shelfH = 0;
    }
    // A pod taller than the room left keeps as many rows as fit.
    const room = floor - y - POD_LABEL_H - 2 * POD_PAD;
    rows = Math.min(rows, Math.floor(room / SLOT_H));
    if (rows < 1) {
      hiddenPods = pods.length - i;
      break;
    }
    const podH = rows * SLOT_H + 2 * POD_PAD + POD_LABEL_H;
    const seats: SeatBox[] = [];
    const shown = Math.min(pod.crew.length, rows * cols);
    for (let j = 0; j < shown; j++) {
      const sx = x + POD_PAD + (j % cols) * SLOT_W + (SLOT_W - LOBSTER_W) / 2;
      const sy = y + POD_LABEL_H + POD_PAD + Math.floor(j / cols) * SLOT_H;
      const nShrimp = Math.min(MAX_SHRIMP, pod.crew[j].subagents.length);
      const shrimp = Array.from({ length: nShrimp }, (_, k) => ({
        x: sx + LOBSTER_W / 2 - (nShrimp * SHRIMP_W) / 2 + k * SHRIMP_W,
        y: sy + LOBSTER_H + 1,
      }));
      seats.push({ member: pod.crew[j], x: sx, y: sy, shrimp });
    }
    shelf.push({ key: pod.key, x, y, w: podW, h: podH, seats, more: pod.crew.length - shown });
    x += podW + GAP;
    shelfH = Math.max(shelfH, podH);
  }
  closeShelf();
  return { pods: out, hiddenPods };
}

function shift(p: PodBox, dx: number, dy: number) {
  p.x += dx;
  p.y += dy;
  for (const s of p.seats) {
    s.x += dx;
    s.y += dy;
    for (const sh of s.shrimp) {
      sh.x += dx;
      sh.y += dy;
    }
  }
}
