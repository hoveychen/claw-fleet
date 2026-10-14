/**
 * Procedural pixel spaceships, one per workspace on the fleet overview. Side
 * view, nose to the right. The hull is generated from a profile rather than
 * hand-drawn so window count and paint can vary per ship; like the lobsters,
 * composition is pure (a grid of palette keys) and rasterised once per look.
 */

import { rasterise, shade } from "./sprites";

export const SHIP_W = 58;
export const SHIP_H = 24;
/** Porthole slots along the hull; crew beyond this are a DOM count. */
export const SHIP_WINDOWS = 7;
/** Column of the canopy's centre, where overlays point at the ship. */
export const SHIP_CANOPY_X = 45;

/** Paint jobs, picked per workspace so neighbouring ships differ. */
export const SHIP_PAINTS = ["#d9452b", "#3a8fd9", "#e0a43a", "#4fb37a", "#a467d6", "#d95a8e"];

const BASE: Record<string, string> = {
  h: "#e4e8ef", // hull highlight
  H: "#b9c0cc", // hull
  D: "#7c8494", // hull shadow
  g: "#7fd8f0", // canopy glass
  G: "#e6fbff", // canopy glint
  w: "#1a2238", // dark porthole
  e: "#4a4f5c", // engine nozzle
  y: "#fff1a8", // flame core
  o: "#ff9a3c", // flame
};

export interface ShipSpec {
  /** Lit porthole colours, left to right; unlit slots stay dark. */
  windows: string[];
  paint: string;
  /** Engines burning (someone aboard is at work). */
  thrust: boolean;
  frame: number;
}

const CY = 12;
const TAIL = 9;
const NOSE = 56;

/** Half-height of the hull at column x (0 = outside the hull). */
function halfHeight(x: number): number {
  if (x < TAIL || x > NOSE) return 0;
  if (x < TAIL + 3) return 4;
  if (x < 40) return 6;
  return Math.max(1, Math.round((6 * (NOSE - x)) / (NOSE - 40)));
}

export function composeShip(spec: ShipSpec): string[] {
  const g = Array.from({ length: SHIP_H }, () => Array.from({ length: SHIP_W }, () => "."));
  const put = (x: number, y: number, ch: string) => {
    if (y >= 0 && y < SHIP_H && x >= 0 && x < SHIP_W) g[y][x] = ch;
  };

  // Tail fins, top and bottom, painted.
  for (let k = 0; k < 5; k++) {
    for (let x = TAIL + k; x < TAIL + 9 - k; x++) {
      put(x, CY - 7 - k, "a");
      put(x, CY + 7 + k, "A");
    }
  }
  // Hull, shaded top to bottom.
  for (let x = TAIL; x <= NOSE; x++) {
    const hh = halfHeight(x);
    for (let y = CY - hh; y <= CY + hh; y++) {
      put(x, y, y === CY - hh ? "h" : y >= CY + hh - 1 ? "D" : "H");
    }
  }
  // Paint stripe along the belly.
  for (let x = TAIL + 2; x < 46; x++) if (halfHeight(x) >= 4) put(x, CY + 3, "a");
  // Canopy near the nose.
  for (let x = 41; x < 50; x++) {
    const top = CY - halfHeight(x) + 1;
    for (let y = top; y < CY; y++) put(x, y, "g");
  }
  put(43, CY - 3, "G");
  put(44, CY - 3, "G");
  // Portholes: 2x2 each.
  for (let i = 0; i < SHIP_WINDOWS; i++) {
    const x = TAIL + 4 + i * 4;
    const ch = i < spec.windows.length ? String(i) : "w";
    for (const [dx, dy] of [[0, -1], [1, -1], [0, 0], [1, 0]]) put(x + dx, CY + dy, ch);
  }
  // Engine nozzle and flame.
  for (let y = CY - 3; y <= CY + 3; y++) {
    put(TAIL - 1, y, "e");
    put(TAIL - 2, y, "e");
  }
  if (spec.thrust) {
    const len = spec.frame % 2 ? 6 : 4;
    for (let x = TAIL - 3; x >= TAIL - 2 - len; x--) {
      const d = TAIL - 3 - x;
      const half = d < len - 2 ? 2 : 1;
      for (let y = CY - half; y <= CY + half; y++) put(x, y, Math.abs(y - CY) < 1 && d < len - 1 ? "y" : "o");
    }
  }
  return g.map((r) => r.join(""));
}

const cache = new Map<string, HTMLCanvasElement>();

export function shipSprite(spec: ShipSpec): HTMLCanvasElement {
  const frame = spec.thrust ? spec.frame % 2 : 0;
  const key = `${spec.paint}|${spec.thrust ? 1 : 0}|${frame}|${spec.windows.join(",")}`;
  let c = cache.get(key);
  if (!c) {
    const hexOf = (ch: string) => {
      if (ch >= "0" && ch <= "9") return spec.windows[Number(ch)];
      if (ch === "a") return spec.paint;
      if (ch === "A") return shade(spec.paint, 0.7);
      return BASE[ch] ?? "#ff00ff";
    };
    c = rasterise(composeShip({ ...spec, frame }), hexOf, "#05060d");
    // Bounded: windows vary with every status change.
    if (cache.size > 400) cache.clear();
    cache.set(key, c);
  }
  return c;
}

/** Stable paint for a workspace name. */
export function paintFor(key: string): string {
  let h = 0;
  for (let i = 0; i < key.length; i++) h = (h * 31 + key.charCodeAt(i)) | 0;
  return SHIP_PAINTS[Math.abs(h) % SHIP_PAINTS.length];
}
