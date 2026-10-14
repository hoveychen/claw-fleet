/**
 * Procedural pixel-art crew in the likeness of the Claw Fleet mascot — a
 * lobster robot with a CRT-screen face, a captain's cap and a navy uniform.
 * Main sessions are full-size captains; sub-agents are small cap-less cadets
 * (still called "shrimp" in the layout code).
 *
 * Sprites are drawn SPRITE_DETAIL times finer than the scenery's art pixel
 * (the engine's canvas carries the extra resolution), so a captain fills a
 * 32x30 seat with a 64x60 drawing. A sprite is first composed as a grid of
 * palette keys from simple shapes — rounded rects, discs, thick lines and a
 * few hand-drawn stamps — then rasterised once, outlined, and cached. The
 * screen's eyes and mouth glow in the state colour. The composer is pure so
 * tests can assert on the grid without a canvas. Approach after
 * rezored/cubicle-crew (MIT); the art is original.
 */

import { SCENE_DETAIL } from "./engine";

/** Everything a crew member can be doing on screen. */
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

/** Footprints in scenery art pixels — what the layout reserves. */
export const LOBSTER_W = 32;
export const LOBSTER_H = 30;
export const SHRIMP_W = 16;
export const SHRIMP_H = 14;
/** Sprite pixels per scenery art pixel. */
export const SPRITE_DETAIL = SCENE_DETAIL;

/** Palette keys → colours. `S`/`Z` are the screen glow (state colour and its
 *  scanline shade), filled per sprite. */
const PALETTE: Record<string, string> = {
  r: "#d9452b", // shell
  R: "#a52a1a", // shell shadow
  h: "#f58a66", // shell highlight
  l: "#b3311d", // legs
  c: "#e5532f", // claw
  C: "#a92e1b", // claw shadow
  k: "#121318", // screen
  n: "#2c303d", // screen glare
  u: "#2a3760", // uniform
  U: "#1c2645", // uniform shadow
  b: "#11172a", // belt
  G: "#ecc04a", // gold trim
  g: "#a8771c", // gold shadow
  W: "#e9edf5", // chest badge
  // Captain's cap
  H: "#1d2130",
  j: "#323950", // cap sheen
  v: "#2a3042", // visor
  // Props
  K: "#5b6170", // keyboard body
  q: "#c9ced8", // keycap
  P: "#f3ecd8", // paper
  i: "#8a7d66", // ink
  M: "#f4f1ea", // mug
  m: "#6b3e22", // coffee
};
const OUTLINE = "#0d0a12";

type Grid = string[][];

const blank = (w: number, h: number): Grid =>
  Array.from({ length: h }, () => Array.from({ length: w }, () => "."));

function put(g: Grid, x: number, y: number, ch: string) {
  if (y >= 0 && y < g.length && x >= 0 && x < g[0].length) g[y][x] = ch;
}

/** Stamp `map` at (x, y); '.' cells are transparent. `flip` mirrors it. */
function stamp(g: Grid, map: string[], x: number, y: number, flip = false) {
  for (let r = 0; r < map.length; r++) {
    const row = map[r];
    for (let c = 0; c < row.length; c++) {
      const ch = flip ? row[row.length - 1 - c] : row[c];
      if (ch !== ".") put(g, x + c, y + r, ch);
    }
  }
}

/** Fill the inclusive rect (x0,y0)–(x1,y1), corners rounded to radius `r`.
 *  `over` limits the fill to cells currently holding one of its keys. */
function rect(g: Grid, x0: number, y0: number, x1: number, y1: number, ch: string, r = 0, over?: string) {
  for (let y = y0; y <= y1; y++) {
    for (let x = x0; x <= x1; x++) {
      if (r > 0) {
        const cx = Math.min(Math.max(x, x0 + r), x1 - r);
        const cy = Math.min(Math.max(y, y0 + r), y1 - r);
        if ((x - cx) ** 2 + (y - cy) ** 2 > r * r + r * 0.5) continue;
      }
      if (over && !over.includes(g[y]?.[x] ?? "")) continue;
      put(g, x, y, ch);
    }
  }
}

/** Fill an ellipse centred on (cx, cy) in cell-edge coordinates. */
function disc(g: Grid, cx: number, cy: number, rx: number, ry: number, ch: string, over?: string) {
  for (let y = Math.floor(cy - ry); y <= Math.ceil(cy + ry); y++) {
    for (let x = Math.floor(cx - rx); x <= Math.ceil(cx + rx); x++) {
      const dx = (x + 0.5 - cx) / rx;
      const dy = (y + 0.5 - cy) / ry;
      if (dx * dx + dy * dy > 1) continue;
      if (over && !over.includes(g[y]?.[x] ?? "")) continue;
      put(g, x, y, ch);
    }
  }
}

/** Bresenham line with a `size`×`size` brush; only paints empty cells, so a
 *  sleeve drawn after the body tucks in behind it. */
function line(g: Grid, x0: number, y0: number, x1: number, y1: number, ch: string, size: number) {
  const dx = Math.abs(x1 - x0);
  const dy = -Math.abs(y1 - y0);
  const sx = x0 < x1 ? 1 : -1;
  const sy = y0 < y1 ? 1 : -1;
  let err = dx + dy;
  const lo = -Math.floor((size - 1) / 2);
  for (;;) {
    for (let oy = lo; oy < lo + size; oy++) {
      for (let ox = lo; ox < lo + size; ox++) {
        if (g[y0 + oy]?.[x0 + ox] === ".") g[y0 + oy][x0 + ox] = ch;
      }
    }
    if (x0 === x1 && y0 === y1) break;
    const e2 = 2 * err;
    if (e2 >= dy) { err += dy; x0 += sx; }
    if (e2 <= dx) { err += dx; y0 += sy; }
  }
}

/** Copy the left half of rows y0..y1 onto the right half, mirrored. */
function mirrorRows(g: Grid, y0: number, y1: number) {
  const w = g[0].length;
  for (let y = Math.max(0, y0); y <= Math.min(g.length - 1, y1); y++) {
    for (let x = 0; x < w / 2; x++) g[y][w - 1 - x] = g[y][x];
  }
}

// ── Captain (64x60) ─────────────────────────────────────────────────────────

const CAP_W = LOBSTER_W * SPRITE_DETAIL;
const CAP_H = LOBSTER_H * SPRITE_DETAIL;

// Big claws, drawn for the left side (opening up and out); flipped for the right.
const CLAW_OPEN = [
  ".cc......cc.",
  "chcc....ccCc",
  "chcc....cCCc",
  "chhcc..ccCCc",
  ".chccccccCc.",
  ".chcccccCCc.",
  "..chcccCCc..",
  "..cccccCCc..",
  "...ccccCc...",
  "....cCCc....",
  "....cCc.....",
  ".....cc.....",
];
const CLAW_SHUT = [
  "....cccc....",
  "..cchhcccc..",
  ".chhccccccc.",
  ".chccccC.Cc.",
  "chcccccCCcCc",
  "chccccCcCCCc",
  ".chcccCCCCc.",
  ".chccccCCCc.",
  "..cccccCCc..",
  "...ccCCCc...",
  "....cCCc....",
  ".....cc.....",
];
const CLAW = 12;

const STAR = ["...GG...", "...GG...", "GGGGGGGG", ".GGGGGG.", "..GGGG..", ".GGGGGG.", ".GG..GG."];

/** Rows from the head's top to the uniform's collar. */
const TORSO_DY = 21;
const HEAD_Y = 10;
const SHOULDER_L = { x: 17, y: TORSO_DY + 3 };
const SHOULDER_R = { x: CAP_W - 1 - 17, y: TORSO_DY + 3 };

/** One claw: top-left of the 12x12 claw map, and whether it is open. */
interface ClawAt { x: number; y: number; open: boolean }
type Eyes = "open" | "closed" | "x" | "up";
interface Rig {
  left: ClawAt;
  right: ClawAt;
  bodyDy: number;
  eyes: Eyes;
  frown?: boolean;
  prop?: "keyboard" | "paper" | "mug";
}

/** Where each pose puts the claws, frame by frame. */
function rigFor(pose: Pose, f: number): Rig {
  const rest = (dy = 0): Rig => ({
    left: { x: 1, y: 34 + dy, open: false },
    right: { x: 51, y: 34 + dy, open: false },
    bodyDy: dy,
    eyes: "open",
  });
  switch (pose) {
    case "idle":
      return { ...rest(f), eyes: f ? "closed" : "open" };
    case "think":
      return { ...rest(), left: { x: 14, y: 24 + f, open: false }, eyes: "up" };
    case "type":
      return {
        left: { x: 12, y: f ? 40 : 37, open: false },
        right: { x: 40, y: f ? 37 : 40, open: false },
        bodyDy: 0,
        eyes: "open",
        prop: "keyboard",
      };
    case "read":
      return {
        left: { x: 8, y: 32 + f, open: false },
        right: { x: 44, y: 32 + f, open: false },
        bodyDy: 0,
        eyes: "closed",
        prop: "paper",
      };
    case "run":
      return { left: { x: 1, y: 20, open: f === 0 }, right: { x: 51, y: 20, open: f === 1 }, bodyDy: 0, eyes: "open" };
    case "delegate":
      return { ...rest(), right: { x: f ? 51 : 52, y: f ? 8 : 4, open: true } };
    case "wave":
      return { left: { x: 0, y: f ? 10 : 6, open: true }, right: { x: 52, y: f ? 6 : 10, open: true }, bodyDy: 0, eyes: "open" };
    case "stuck":
      return { left: { x: 4, y: 46, open: false }, right: { x: 48, y: 46, open: false }, bodyDy: 2, eyes: "closed", frown: true };
    case "coffee":
      return { ...rest(), right: { x: 40, y: 28 + f, open: false }, prop: "mug" };
    case "error":
      return {
        ...rest(),
        left: { x: 1, y: 34 - 2 * f, open: true },
        right: { x: 51, y: 32 + 2 * f, open: true },
        eyes: "x",
        frown: true,
      };
    case "watch":
      return { ...rest(f), eyes: "up" };
  }
}

export interface LobsterSpec {
  pose: Pose;
  frame: number;
  /** Screen glow (hex) — the state colour. */
  color: string;
  captain: boolean;
}

/** Eyes centred on these columns (cell edges); mirrored about the middle. */
const EYE_L = 26;
const EYE_R = CAP_W - EYE_L;

/** The screen's eyes and mouth, glowing in `S` with `Z` scanlines. */
function drawFace(g: Grid, by: number, eyes: Eyes, frown: boolean) {
  for (const cx of [EYE_L, EYE_R]) {
    const x0 = cx - 3;
    switch (eyes) {
      case "open":
      case "up": {
        const cy = by + (eyes === "up" ? 7 : 9);
        disc(g, cx, cy, 3, 3, "S", "k");
        for (let y = cy - 3; y < cy + 3; y++) if ((y - by) % 2) rect(g, x0, y, x0 + 5, y, "Z", 0, "S");
        break;
      }
      case "closed":
        // A happy arc, like an eye squeezed shut.
        rect(g, x0 + 1, by + 9, x0 + 4, by + 9, "S");
        for (const x of [x0, x0 + 1, x0 + 4, x0 + 5]) put(g, x, by + 10, "S");
        break;
      case "x":
        for (let i = 0; i < 6; i++) {
          put(g, x0 + i, by + 6 + i, "S");
          put(g, x0 + 5 - i, by + 6 + i, "S");
        }
        break;
    }
  }
  const mid = CAP_W / 2;
  const [edge, belly] = frown ? [by + 14, by + 13] : [by + 13, by + 14];
  put(g, mid - 4, edge, "S");
  put(g, mid + 3, edge, "S");
  rect(g, mid - 3, belly, mid + 2, belly, "S");
}

/** Peaked cap with a gold star and band; its visor shades the screen's top. */
function drawCap(g: Grid, by: number) {
  const y = by - 9;
  const crown: [number, number][] = [[19, 44], [16, 47], [14, 49], [14, 49], [14, 49], [14, 49], [15, 48], [16, 47]];
  crown.forEach(([a, b], i) => rect(g, a, y + i, b, y + i, "H"));
  rect(g, 18, y + 1, 29, y + 1, "j");
  rect(g, 16, y + 2, 21, y + 2, "j");
  stamp(g, STAR, CAP_W / 2 - 4, y);
  rect(g, 15, y + 8, 48, y + 8, "G");
  rect(g, 15, y + 9, 48, y + 9, "g");
  rect(g, 15, y + 10, 48, y + 10, "j");
  rect(g, 16, y + 11, 47, y + 11, "v");
  rect(g, 18, y + 12, 45, y + 12, "v");
}

/** Epaulettes with a fringe; drawn over the sleeves' tops. */
function drawEpaulettes(g: Grid, by: number) {
  const ty = by + TORSO_DY;
  for (const x0 of [12, 43]) {
    rect(g, x0, ty - 1, x0 + 8, ty + 3, "G", 2);
    rect(g, x0, ty + 3, x0 + 8, ty + 3, "g", 0, "G");
    for (let x = x0 + 1; x <= x0 + 7; x += 2) rect(g, x, ty + 4, x, ty + 5, "G");
  }
}

/** Head, uniform, belt and legs — everything that does not move with a pose. */
function drawBody(g: Grid, by: number) {
  // Head: a red shell holding a black screen.
  rect(g, 15, by, 48, by + 20, "r", 7);
  rect(g, 15, by + 18, 48, by + 20, "R", 0, "r");
  disc(g, 21, by + 5, 3, 2, "h", "r");
  rect(g, 18, by + 4, 45, by + 17, "k", 4);
  put(g, 21, by + 6, "n");
  put(g, 22, by + 6, "n");
  put(g, 21, by + 7, "n");

  const ty = by + TORSO_DY;
  // Uniform: navy, shaded at the sides and hem.
  rect(g, 16, ty, 47, ty + 17, "u", 4);
  rect(g, 16, ty, 17, ty + 17, "U", 0, "u");
  rect(g, 46, ty, 47, ty + 17, "U", 0, "u");
  rect(g, 16, ty + 16, 47, ty + 17, "U", 0, "u");
  // Collar: a red V of neck edged in gold.
  for (let i = 0; i < 5; i++) {
    rect(g, 27 + i, ty + i, 36 - i, ty + i, "r");
    put(g, 26 + i, ty + i, "G");
    put(g, 37 - i, ty + i, "G");
  }
  // Double-breasted buttons.
  for (const row of [7, 10, 13]) {
    for (const x of [27, 35]) {
      rect(g, x, ty + row, x + 1, ty + row, "G");
      rect(g, x, ty + row + 1, x + 1, ty + row + 1, "g");
    }
  }
  // Belt and buckle.
  rect(g, 16, ty + 15, 47, ty + 16, "b", 0, "uU");
  rect(g, 28, ty + 14, 35, ty + 17, "G");
  rect(g, 30, ty + 15, 33, ty + 16, "b");
  // Chest badge.
  disc(g, 42, ty + 7, 2.6, 2.6, "W");
  disc(g, 42, ty + 7, 1.5, 1.5, "u");
  put(g, 42, ty + 6, "r");

  // Red lower shell and three pairs of legs.
  rect(g, 18, ty + 18, 45, ty + 21, "r", 3);
  rect(g, 18, ty + 21, 45, ty + 21, "R", 0, "r");
  line(g, 21, ty + 22, 18, ty + 26, "l", 2);
  line(g, 25, ty + 22, 23, ty + 26, "l", 2);
  line(g, 29, ty + 22, 28, ty + 26, "l", 2);
  mirrorRows(g, ty + 22, ty + 26);
}

function drawKeyboard(g: Grid) {
  rect(g, 12, 51, 51, 58, "K", 1);
  for (let x = 14; x <= 48; x += 3) rect(g, x, 53, x + 1, 53, "q");
  for (let x = 15; x <= 47; x += 3) rect(g, x, 55, x + 1, 55, "q");
  rect(g, 24, 57, 39, 57, "q");
}

function drawPaper(g: Grid, by: number) {
  const y = by + 22;
  rect(g, 20, y, 43, y + 13, "P");
  for (const [row, len] of [[2, 18], [5, 14], [8, 17], [11, 10]] as const) rect(g, 23, y + row, 22 + len, y + row, "i");
}

function drawMug(g: Grid, x: number, y: number) {
  rect(g, x, y, x + 6, y + 8, "M", 1);
  rect(g, x + 1, y + 1, x + 5, y + 1, "m");
  rect(g, x + 7, y + 2, x + 8, y + 2, "M");
  rect(g, x + 8, y + 3, x + 8, y + 5, "M");
  rect(g, x + 7, y + 6, x + 8, y + 6, "M");
}

/** Compose a captain as a grid of palette keys (pure; no canvas). */
export function composeLobster(spec: LobsterSpec): string[] {
  const g = blank(CAP_W, CAP_H);
  const rig = rigFor(spec.pose, spec.frame % POSE_FRAMES[spec.pose].n);
  const by = HEAD_Y + rig.bodyDy;

  drawBody(g, by);
  drawFace(g, by, rig.eyes, !!rig.frown);
  if (spec.captain) drawCap(g, by);

  if (rig.prop === "paper") drawPaper(g, by);
  for (const [claw, shoulder, flip] of [
    [rig.left, SHOULDER_L, false],
    [rig.right, SHOULDER_R, true],
  ] as const) {
    // Aim the sleeve at the claw's centre; the claw is stamped over the
    // sleeve's end, so the joint looks right whether it is raised or lowered.
    // Sleeves go first so the epaulettes and claws sit on top of them.
    line(g, shoulder.x, shoulder.y + by, claw.x + CLAW / 2 - (flip ? 0 : 1), claw.y + CLAW / 2, "u", 3);
  }
  drawEpaulettes(g, by);
  for (const [claw, flip] of [
    [rig.left, false],
    [rig.right, true],
  ] as const) {
    stamp(g, claw.open ? CLAW_OPEN : CLAW_SHUT, claw.x, claw.y, flip);
  }
  if (rig.prop === "keyboard") drawKeyboard(g);
  if (rig.prop === "mug") drawMug(g, rig.right.x - 8, rig.right.y + 2);

  return g.map((row) => row.join(""));
}

// ── Cadet (32x28) ───────────────────────────────────────────────────────────

const CADET_W = SHRIMP_W * SPRITE_DETAIL;
const CADET_H = SHRIMP_H * SPRITE_DETAIL;
const CADET_CLAW = ["c....c", "cc..cc", "chccCc", "chcCCc", ".cCCc.", "..cc.."];
/** Two frames: claws bob and the legs step. */
const SHRIMP_FRAMES = 2;

export interface ShrimpSpec {
  frame: number;
  color: string;
}

export function composeShrimp(spec: ShrimpSpec): string[] {
  const f = spec.frame % SHRIMP_FRAMES;
  const g = blank(CADET_W, CADET_H);
  // Head and screen face.
  rect(g, 7, 1, 24, 12, "r", 4);
  rect(g, 7, 12, 24, 12, "R", 0, "r");
  put(g, 10, 3, "h");
  put(g, 11, 3, "h");
  put(g, 10, 4, "h");
  rect(g, 9, 3, 22, 10, "k", 2);
  for (const x of [11, 18]) {
    rect(g, x, 5, x + 2, 7, "S");
    rect(g, x, 6, x + 2, 6, "Z");
  }
  put(g, 13, 8, "S");
  put(g, 18, 8, "S");
  rect(g, 14, 9, 17, 9, "S");
  // Uniform.
  rect(g, 9, 13, 22, 21, "u", 2);
  rect(g, 9, 20, 22, 21, "U", 0, "u");
  rect(g, 14, 13, 17, 13, "r");
  rect(g, 15, 14, 16, 14, "r");
  for (const [x, y] of [[13, 13], [18, 13], [14, 14], [17, 14]]) put(g, x, y, "G");
  for (const y of [16, 18]) for (const x of [13, 18]) put(g, x, y, "G");
  rect(g, 9, 19, 22, 19, "b", 0, "uU");
  rect(g, 15, 19, 16, 19, "G");
  rect(g, 7, 13, 10, 15, "G", 1);
  rect(g, 21, 13, 24, 15, "G", 1);
  // Lower shell and stepping legs.
  rect(g, 10, 22, 21, 23, "r", 1);
  if (f === 0) {
    line(g, 12, 24, 10, 27, "l", 1);
    line(g, 15, 24, 14, 27, "l", 1);
  } else {
    line(g, 12, 24, 11, 27, "l", 1);
    line(g, 15, 24, 15, 27, "l", 1);
  }
  mirrorRows(g, 24, 27);
  // Claws, waving a little.
  const cy = f ? 11 : 9;
  line(g, 9, 15, 4, cy + 4, "u", 2);
  line(g, 22, 15, 27, cy + 4, "u", 2);
  stamp(g, CADET_CLAW, 1, cy);
  stamp(g, CADET_CLAW, 25, cy, true);
  return g.map((row) => row.join(""));
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

/** Palette lookup for crew sprites: `S`/`Z` are the screen glow and its scanline. */
const creatureColours = (glow: string) => (ch: string) =>
  ch === "S" ? glow : ch === "Z" ? shade(glow, 0.72) : PALETTE[ch] ?? "#ff00ff";

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
  const frame = spec.frame % SHRIMP_FRAMES;
  const key = `S|${frame}|${spec.color}`;
  let c = cache.get(key);
  if (!c) {
    c = rasterise(composeShrimp({ ...spec, frame }), creatureColours(spec.color));
    cache.set(key, c);
  }
  return c;
}

/** Current frame of `pose` at time `t` (ms), offset per crew member so a crowd
 *  does not move in lockstep. */
export function frameAt(pose: Pose, t: number, phase: number): number {
  const { n, fps } = POSE_FRAMES[pose];
  return Math.floor((t / 1000) * fps + phase) % n;
}
