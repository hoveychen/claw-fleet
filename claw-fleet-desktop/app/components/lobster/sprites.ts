/**
 * Procedural pixel-art lobsters (captains) and shrimp (sub-agent crew).
 *
 * A sprite is first composed as a grid of palette keys — body halves mirrored
 * for a symmetric front view, claws stamped where the pose puts them, arms
 * drawn as lines from the shoulder — then rasterised once, outlined, and
 * cached. The composer is pure so tests can assert on the grid without a
 * canvas. Approach after rezored/cubicle-crew (MIT); the art is original.
 */

/** Everything a lobster can be doing on screen. */
export type Pose =
  | "idle"
  | "think"
  | "type"
  | "read"
  | "run"
  | "delegate"
  | "wave"
  | "stuck"
  | "coffee"
  | "error"
  | "watch";

/** Frames per pose and how fast they cycle (frames per second). */
export const POSE_FRAMES: Record<Pose, { n: number; fps: number }> = {
  idle: { n: 2, fps: 1 },
  think: { n: 2, fps: 1.5 },
  type: { n: 2, fps: 6 },
  read: { n: 2, fps: 1 },
  run: { n: 2, fps: 5 },
  delegate: { n: 2, fps: 3 },
  wave: { n: 2, fps: 4 },
  stuck: { n: 1, fps: 1 },
  coffee: { n: 2, fps: 0.7 },
  error: { n: 2, fps: 3 },
  watch: { n: 2, fps: 0.5 },
};

export const LOBSTER_W = 32;
export const LOBSTER_H = 30;
export const SHRIMP_W = 16;
export const SHRIMP_H = 14;

/** Palette keys → colours. `S`/`Z` are the state scarf, filled per sprite. */
const PALETTE: Record<string, string> = {
  r: "#d9452b", // shell
  R: "#a52a1a", // shell shadow
  h: "#f27a55", // shell highlight
  t: "#c43a24", // tail fan
  T: "#8f2414", // tail fan shadow
  l: "#9b2717", // legs
  c: "#e5532f", // claw
  C: "#b23420", // claw shadow
  k: "#141018", // eye
  w: "#ffffff", // eye glint
  a: "#f08a6a", // antenna
  // Captain's cap
  H: "#f4f1ea",
  B: "#1d2433",
  G: "#e8b93a",
  // Props
  K: "#5b6170", // keyboard body
  q: "#c9ced8", // keycap
  P: "#f3ecd8", // paper
  i: "#6d6250", // ink
  M: "#f4f1ea", // mug
  m: "#6b3e22", // coffee
  // Shrimp
  p: "#f59a80",
  n: "#d8694f",
};
const OUTLINE = "#2a0d0a";

type Grid = string[][];

const blank = (w: number, h: number): Grid =>
  Array.from({ length: h }, () => Array.from({ length: w }, () => "."));

/** Stamp `map` at (x, y); '.' cells are transparent. `flip` mirrors it. */
function stamp(g: Grid, map: string[], x: number, y: number, flip = false) {
  for (let r = 0; r < map.length; r++) {
    const row = map[r];
    for (let c = 0; c < row.length; c++) {
      const ch = flip ? row[row.length - 1 - c] : row[c];
      if (ch === ".") continue;
      const gy = y + r;
      const gx = x + c;
      if (gy >= 0 && gy < g.length && gx >= 0 && gx < g[0].length) g[gy][gx] = ch;
    }
  }
}

/** Mirror each left-half row into a full symmetric row. */
const mirror = (half: string[]) => half.map((row) => row + [...row].reverse().join(""));

// Front-facing lobster body, left half (8 cols → 16 wide). Row 8 is the scarf.
const BODY = mirror([
  "...kk...",
  "...wk...",
  "....r...",
  "....rr..",
  "....hrrr",
  "...hrrrr",
  "...rrrrr",
  "...rrrrr",
  "...SSSSS",
  ".l.rZrrr",
  "l..RRrrr",
  ".l.rrrrr",
  "l...RRrr",
  "....rrrr",
  "....Rrrr",
  "...ttrrr",
  "..tttTrr",
  "..tTt.tT",
]);
const BODY_X = 8;
const BODY_Y = 8;

const EYES_CLOSED = mirror(["........", "...kk..."]);
const EYES_X = mirror(["...wk...", "...kw..."]);
const EYES_UP = mirror(["...wk...", "...kk..."]);

const CLAW_OPEN = ["c....c", "cc..cc", "cCccCc", "cCCCCc", ".cCCc.", "..cc.."];
const CLAW_SHUT = ["..cc..", ".cCcc.", "cCCcCc", "cCCCCc", ".cCCc.", "..cc.."];

const CAP = ["..HHHH..", ".HHGGHH.", "HHHHHHHH", "BBBBBBBB"];
const KEYBOARD = [
  "KKKKKKKKKKKKKKKKKKKK",
  "KqKqKqKqKqKqKqKqKqKK",
  "KKqKqKqKqKqKqKqKqKqK",
  "KKKKKKKKKKKKKKKKKKKK",
];
const PAPER = ["PPPPPPPPPPP", "PiiiPiiPiiP", "PPPPPPPPPPP", "PiiPiiiiPPP", "PPPPPPPPPPP", "PiiiiPiiPPP", "PPPPPPPPPPP"];
const MUG = ["MMMM.", "MmmMM", "MmmM.M", "MMMMM", ".MMM."];

const SHOULDER_L = { x: 11, y: 17 };
const SHOULDER_R = { x: 20, y: 17 };

/** Bresenham line of `ch`, two pixels thick (arm). */
function line(g: Grid, x0: number, y0: number, x1: number, y1: number, ch: string) {
  const dx = Math.abs(x1 - x0);
  const dy = -Math.abs(y1 - y0);
  const sx = x0 < x1 ? 1 : -1;
  const sy = y0 < y1 ? 1 : -1;
  let err = dx + dy;
  for (;;) {
    for (const [ox, oy] of [[0, 0], [0, 1]]) {
      const gx = x0 + ox;
      const gy = y0 + oy;
      if (gy >= 0 && gy < g.length && gx >= 0 && gx < g[0].length && g[gy][gx] === ".") g[gy][gx] = ch;
    }
    if (x0 === x1 && y0 === y1) break;
    const e2 = 2 * err;
    if (e2 >= dy) { err += dy; x0 += sx; }
    if (e2 <= dx) { err += dx; y0 += sy; }
  }
}

/** One claw: top-left of the 6x6 claw map, and whether it is open. */
interface ClawAt { x: number; y: number; open: boolean }
interface Rig {
  left: ClawAt;
  right: ClawAt;
  bodyDy: number;
  eyes: "open" | "closed" | "x" | "up";
  prop?: "keyboard" | "paper" | "mug";
}

/** Where each pose puts the claws, frame by frame. */
function rigFor(pose: Pose, f: number): Rig {
  const rest = (dy = 0): Rig => ({
    left: { x: 2, y: 18 + dy, open: false },
    right: { x: 24, y: 18 + dy, open: false },
    bodyDy: dy,
    eyes: "open",
  });
  switch (pose) {
    case "idle":
      return { ...rest(f), eyes: f ? "closed" : "open" };
    case "think":
      return { ...rest(), left: { x: 9, y: 12 + f, open: false } };
    case "type":
      return {
        left: { x: 7, y: f ? 21 : 19, open: false },
        right: { x: 19, y: f ? 19 : 21, open: false },
        bodyDy: 0,
        eyes: "open",
        prop: "keyboard",
      };
    case "read":
      return {
        left: { x: 4, y: 15 + f, open: false },
        right: { x: 22, y: 15 + f, open: false },
        bodyDy: 0,
        eyes: "closed",
        prop: "paper",
      };
    case "run":
      return { left: { x: 1, y: 10, open: f === 0 }, right: { x: 25, y: 10, open: f === 1 }, bodyDy: 0, eyes: "open" };
    case "delegate":
      return { ...rest(), right: { x: f ? 25 : 26, y: f ? 5 : 3, open: true } };
    case "wave":
      return { left: { x: 0, y: f ? 5 : 3, open: true }, right: { x: 26, y: f ? 3 : 5, open: true }, bodyDy: 0, eyes: "open" };
    case "stuck":
      return { left: { x: 3, y: 23, open: false }, right: { x: 23, y: 23, open: false }, bodyDy: 3, eyes: "closed" };
    case "coffee":
      return { ...rest(), right: { x: 20, y: 13 + f, open: false }, prop: "mug" };
    case "error":
      return { ...rest(), left: { x: 2, y: 18 - f, open: true }, right: { x: 24, y: 17 + f, open: true }, eyes: "x" };
    case "watch":
      return { ...rest(), eyes: "up", bodyDy: f };
  }
}

export interface LobsterSpec {
  pose: Pose;
  frame: number;
  /** Scarf colour (hex) — the state colour. */
  color: string;
  captain: boolean;
}

/** Compose a lobster as a grid of palette keys (pure; no canvas). */
export function composeLobster(spec: LobsterSpec): string[] {
  const g = blank(LOBSTER_W, LOBSTER_H);
  const rig = rigFor(spec.pose, spec.frame % POSE_FRAMES[spec.pose].n);
  const by = BODY_Y + rig.bodyDy;

  // Antennae: long whiskers sweeping out and up from the head.
  line(g, BODY_X + 4, by + 4, BODY_X - 2, by - 8, "a");
  line(g, BODY_X + 11, by + 4, BODY_X + 17, by - 8, "a");

  stamp(g, BODY, BODY_X, by);
  const eyes = rig.eyes === "closed" ? EYES_CLOSED : rig.eyes === "x" ? EYES_X : rig.eyes === "up" ? EYES_UP : null;
  if (eyes) {
    // Clear the open eyes, then draw the variant.
    for (let r = 0; r < 2; r++) for (let c = 0; c < 16; c++) if ("kw".includes(g[by + r][BODY_X + c])) g[by + r][BODY_X + c] = ".";
    stamp(g, eyes, BODY_X, by);
  }
  if (spec.captain) stamp(g, CAP, BODY_X + 4, by + 1);

  if (rig.prop === "paper") stamp(g, PAPER, BODY_X + 2, by + 9);
  for (const [claw, shoulder, flip] of [
    [rig.left, SHOULDER_L, false],
    [rig.right, SHOULDER_R, true],
  ] as const) {
    // Aim the arm at the claw's centre; the claw is stamped over the arm's
    // end, so the joint looks right whether the claw is raised or lowered.
    line(g, shoulder.x, shoulder.y + rig.bodyDy, claw.x + 2 + (flip ? 1 : 0), claw.y + 2, "r");
    stamp(g, claw.open ? CLAW_OPEN : CLAW_SHUT, claw.x, claw.y, flip);
  }
  if (rig.prop === "keyboard") stamp(g, KEYBOARD, 6, 26);
  if (rig.prop === "mug") stamp(g, MUG, rig.right.x - 4, rig.right.y + 2);

  return g.map((row) => row.join(""));
}

// Shrimp, side view facing right; two frames (tail flick).
const SHRIMP = [
  [
    "................",
    ".......aaaa.....",
    "....pppp...aa...",
    "...ppppkp.......",
    "..pppppppp......",
    "..SSSSpppp......",
    "...pZpppnp......",
    "...pppppnn......",
    "....ppppn.......",
    ".....pppn.......",
    "......pp........",
    "....nppp........",
    "...nn.n.........",
    "................",
  ],
  [
    "................",
    "........aaaa....",
    "....pppp....a...",
    "...ppppkp.......",
    "..pppppppp......",
    "..SSSSpppp......",
    "...pZpppnp......",
    "...pppppnn......",
    "....ppppn.......",
    ".....pppn.......",
    ".....ppp........",
    "....ppnn........",
    "....n.nn........",
    "................",
  ],
];

export interface ShrimpSpec {
  frame: number;
  color: string;
}

export function composeShrimp(spec: ShrimpSpec): string[] {
  return SHRIMP[spec.frame % SHRIMP.length];
}

// ── Rasterising ─────────────────────────────────────────────────────────────

function hexToRgb(hex: string): [number, number, number] {
  const n = parseInt(hex.slice(1), 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}

export function shade(hex: string, k: number): string {
  const [r, g, b] = hexToRgb(hex);
  const f = (v: number) => Math.max(0, Math.min(255, Math.round(v * k)));
  return `#${[f(r), f(g), f(b)].map((v) => v.toString(16).padStart(2, "0")).join("")}`;
}

/** Palette lookup for creature sprites: `S`/`Z` are the scarf and its shadow. */
const creatureColours = (scarf: string) => (ch: string) =>
  ch === "S" ? scarf : ch === "Z" ? shade(scarf, 0.72) : PALETTE[ch] ?? "#ff00ff";

/** Grid → outlined canvas. Any transparent cell touching an opaque one (4-way)
 *  becomes outline, which keeps the sprite readable on any backdrop. */
export function rasterise(rows: string[], hexOf: (ch: string) => string, outlineHex = OUTLINE): HTMLCanvasElement {
  const h = rows.length;
  const w = rows[0].length;
  const canvas = document.createElement("canvas");
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext("2d");
  if (!ctx) return canvas;
  const img = ctx.createImageData(w, h);
  const colours: Record<string, [number, number, number]> = {};
  const colourOf = (ch: string) => {
    if (!colours[ch]) {
      colours[ch] = hexToRgb(hexOf(ch));
    }
    return colours[ch];
  };
  const outline = hexToRgb(outlineHex);
  const opaque = (x: number, y: number) => x >= 0 && y >= 0 && x < w && y < h && rows[y][x] !== ".";
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) {
      let rgb: [number, number, number] | null = null;
      if (opaque(x, y)) rgb = colourOf(rows[y][x]);
      else if (opaque(x - 1, y) || opaque(x + 1, y) || opaque(x, y - 1) || opaque(x, y + 1)) rgb = outline;
      if (!rgb) continue;
      const i = (y * w + x) * 4;
      img.data[i] = rgb[0];
      img.data[i + 1] = rgb[1];
      img.data[i + 2] = rgb[2];
      img.data[i + 3] = 255;
    }
  }
  ctx.putImageData(img, 0, 0);
  return canvas;
}

const cache = new Map<string, HTMLCanvasElement>();

export function lobsterSprite(spec: LobsterSpec): HTMLCanvasElement {
  const frame = spec.frame % POSE_FRAMES[spec.pose].n;
  const key = `L|${spec.pose}|${frame}|${spec.color}|${spec.captain ? 1 : 0}`;
  let c = cache.get(key);
  if (!c) {
    c = rasterise(composeLobster({ ...spec, frame }), creatureColours(spec.color));
    cache.set(key, c);
  }
  return c;
}

export function shrimpSprite(spec: ShrimpSpec): HTMLCanvasElement {
  const frame = spec.frame % SHRIMP.length;
  const key = `S|${frame}|${spec.color}`;
  let c = cache.get(key);
  if (!c) {
    c = rasterise(composeShrimp({ ...spec, frame }), creatureColours(spec.color));
    cache.set(key, c);
  }
  return c;
}

/** Current frame of `pose` at time `t` (ms), offset per lobster so a crowd
 *  does not move in lockstep. */
export function frameAt(pose: Pose, t: number, phase: number): number {
  const { n, fps } = POSE_FRAMES[pose];
  return Math.floor((t / 1000) * fps + phase) % n;
}
