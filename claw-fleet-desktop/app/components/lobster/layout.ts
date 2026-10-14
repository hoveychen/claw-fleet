/**
 * Who is on screen and where. The fleet overview is one ship per workspace;
 * boarding a ship shows its bridge, the workspace's sessions seated at their
 * consoles. Pure (no canvas, no DOM) so the geometry is unit-tested; the view
 * draws whatever boxes this returns.
 */

import type { SessionInfo } from "../../types";
import { isLiveMember } from "../../types";
import { pickScale, SCENE_DETAIL } from "./engine";
import { SHIP_H, SHIP_W } from "./ship";
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
 *  Pods with something live (or a card waiting) come first, then by name —
 *  not by recency, which would reshuffle the whole sea on every transcript write. */
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
  out.sort((a, b) => podRank(b) - podRank(a) || a.key.localeCompare(b.key));
  return out;
}

// ── Geometry (art pixels) ───────────────────────────────────────────────────

/** Strip under each lobster that the DOM name label sits in. */
export const NAME_H = 5;
/** The shrimp sprite's blank top and bottom rows, tucked into the seat. */
const SHRIMP_TRIM = 1;
/** One seat: a lobster, its name strip, and a row of shrimp under that. */
export const SLOT_W = LOBSTER_W + 6;
export const SLOT_H = LOBSTER_H + NAME_H + SHRIMP_H - 3 * SHRIMP_TRIM;
/** Shrimp drawn per seat; the rest are a DOM count badge. */
export const MAX_SHRIMP = 2;
const MARGIN = 6;

export interface SeatBox {
  member: CrewMember;
  /** Top-left of the lobster sprite. */
  x: number;
  y: number;
  /** Top-left of each drawn shrimp. */
  shrimp: { x: number; y: number }[];
}

/** Centre `n` cells of `cw`×`ch` in a `w`-wide band starting at `top`, keeping
 *  only as many rows as end above `bottom`. */
function grid(n: number, cw: number, ch: number, w: number, top: number, bottom: number) {
  const cols = Math.max(1, Math.min(n, Math.floor((w - 2 * MARGIN) / cw)));
  const rows = Math.max(0, Math.min(Math.ceil(n / cols), Math.floor((bottom - top) / ch)));
  const shown = Math.min(n, rows * cols);
  const cells: { x: number; y: number }[] = [];
  for (let i = 0; i < shown; i++) {
    const row = Math.floor(i / cols);
    // The last row may be short; centre it on its own.
    const inRow = Math.min(cols, shown - row * cols);
    const x0 = Math.floor((w - inRow * cw) / 2);
    cells.push({ x: x0 + (i % cols) * cw, y: top + row * ch });
  }
  return { cells, rows, hidden: n - shown };
}

// Overview: one ship per workspace.

/** A ship's cell: the sprite plus room under it for the DOM name plate. */
export const SHIP_CELL_W = SHIP_W + 16;
export const SHIP_CELL_H = SHIP_H + 18;

export interface ShipBox {
  pod: PodIn;
  /** Top-left of the ship sprite. */
  x: number;
  y: number;
}

export interface FleetLayout {
  ships: ShipBox[];
  /** Workspaces that did not fit on screen. */
  hidden: number;
}

/** Grid the ships across the sky, centred both ways. */
export function layoutShips(pods: PodIn[], w: number, h: number): FleetLayout {
  const g = grid(pods.length, SHIP_CELL_W, SHIP_CELL_H, w, MARGIN, h - MARGIN);
  const dy = Math.max(0, Math.floor((h - 2 * MARGIN - g.rows * SHIP_CELL_H) / 2));
  return {
    ships: g.cells.map((c, i) => ({
      pod: pods[i],
      x: c.x + (SHIP_CELL_W - SHIP_W) / 2,
      y: c.y + dy + 4,
    })),
    hidden: g.hidden,
  };
}

// Bridge: the boarded workspace's crew at their consoles.

export interface BridgeLayout {
  /** Height of the forward viewport across the top of the bridge. */
  viewportH: number;
  seats: SeatBox[];
  /** Top of each seat row (the view draws a console bank behind each). */
  rowTops: number[];
  /** Crew that did not fit. */
  more: number;
}

/** Seats sit this far below their row's top so the console screens behind
 *  them show above the crew's caps. */
const CONSOLE_RISE = 4;
const BRIDGE_ROW_H = SLOT_H + CONSOLE_RISE;

export function layoutBridge(crew: CrewMember[], w: number, h: number): BridgeLayout {
  const viewportH = Math.max(36, Math.min(80, Math.round(h * 0.3)));
  const top = viewportH + 8;
  const g = grid(crew.length, SLOT_W, BRIDGE_ROW_H, w, top, h - 2);
  // Centre the rows on the deck so a small crew does not huddle under the
  // viewport with an empty floor below.
  const dy = Math.max(0, Math.floor((h - 2 - top - g.rows * BRIDGE_ROW_H) / 2));
  const seats = g.cells.map((c, i): SeatBox => {
    const sx = c.x + (SLOT_W - LOBSTER_W) / 2;
    const sy = c.y + dy + CONSOLE_RISE;
    const nShrimp = Math.min(MAX_SHRIMP, crew[i].subagents.length);
    return {
      member: crew[i],
      x: sx,
      y: sy,
      shrimp: Array.from({ length: nShrimp }, (_, k) => ({
        x: sx + LOBSTER_W / 2 - (nShrimp * SHRIMP_W) / 2 + k * SHRIMP_W,
        y: sy + LOBSTER_H + NAME_H - SHRIMP_TRIM,
      })),
    };
  });
  const rowTops = Array.from({ length: g.rows }, (_, r) => top + dy + r * BRIDGE_ROW_H);
  return { viewportH, seats, rowTops, more: g.hidden };
}

/** Zoom levels (logical heights) the bridge tries, closest first. */
const BRIDGE_ZOOM = [140, 180];
/** Default height: what the overview, and a crowded bridge, use. */
export const FLEET_TARGET_H = 220;

/** The closest zoom at which the whole crew still fits on a `devW`×`devH`
 *  (device pixels) bridge, so a small crew is drawn large. */
export function bridgeTargetH(crew: CrewMember[], devW: number, devH: number): number {
  for (const target of BRIDGE_ZOOM) {
    const p = pickScale(devW, devH, target, SCENE_DETAIL);
    if (layoutBridge(crew, p.w, p.h).more === 0) return target;
  }
  return FLEET_TARGET_H;
}
