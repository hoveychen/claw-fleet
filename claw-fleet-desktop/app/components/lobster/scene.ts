/**
 * Painted backdrops: open space for the fleet overview, and the inside of a
 * ship's bridge once boarded. Fixed art colours on purpose — this is a
 * painted scene, not themed chrome.
 */

/** Cheap deterministic hash → [0, 1); keeps the sky stable across frames. */
export function hash01(n: number): number {
  const x = Math.sin(n * 127.1 + 311.7) * 43758.5453;
  return x - Math.floor(x);
}

const SKY_TOP = "#060a1c";
const SKY_BOTTOM = "#170b30";
const STAR_TINTS = ["#ffffff", "#cfe0ff", "#ffe9c4"];

/** Stars in three parallax layers drifting left, twinkling. `speed` scales the
 *  drift (the bridge viewport streams them fast while the crew works). */
export function drawStars(
  ctx: CanvasRenderingContext2D,
  t: number,
  x0: number,
  y0: number,
  w: number,
  h: number,
  speed = 1,
  streak = false,
) {
  const count = Math.ceil((w * h) / 140);
  for (let i = 0; i < count; i++) {
    const layer = i % 3;
    const v = (0.4 + layer * 0.8) * speed;
    const x = x0 + Math.floor((((hash01(i) * w - (t / 1000) * v * 6) % w) + w) % w);
    const y = y0 + Math.floor(hash01(i + 7777) * h);
    const tw = 0.45 + 0.55 * Math.abs(Math.sin(t / (900 + layer * 500) + i));
    ctx.globalAlpha = layer === 2 ? tw : tw * 0.6;
    ctx.fillStyle = STAR_TINTS[i % STAR_TINTS.length];
    const len = streak ? 2 + layer * 3 : 1;
    ctx.fillRect(x, y, Math.min(len, x0 + w - x), 1);
  }
  ctx.globalAlpha = 1;
}

/** Open space: gradient sky, dithered nebulae, a ringed planet, stars. */
export function drawSpace(ctx: CanvasRenderingContext2D, t: number, w: number, h: number) {
  const g = ctx.createLinearGradient(0, 0, 0, h);
  g.addColorStop(0, SKY_TOP);
  g.addColorStop(1, SKY_BOTTOM);
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, w, h);

  // Nebulae: sparse dithered clouds, so they read as pixel art, not blur.
  const clouds = [
    { cx: w * 0.22, cy: h * 0.3, r: Math.min(w, h) * 0.32, c: "#6a2c8f" },
    { cx: w * 0.78, cy: h * 0.72, r: Math.min(w, h) * 0.28, c: "#1d5f7a" },
  ];
  for (const cl of clouds) {
    ctx.fillStyle = cl.c;
    for (let y = Math.floor(cl.cy - cl.r); y < cl.cy + cl.r; y++) {
      for (let x = Math.floor(cl.cx - cl.r); x < cl.cx + cl.r; x++) {
        const d = Math.hypot((x - cl.cx) / cl.r, (y - cl.cy) / (cl.r * 0.7));
        if (d >= 1) continue;
        // Ordered dither, denser towards the centre.
        const bayer = ((x & 1) * 2 + (y & 1) * 3) % 4;
        if ((1 - d) * 4 * 0.55 > bayer + hash01(x * 31 + y) * 0.8) {
          ctx.globalAlpha = 0.28;
          ctx.fillRect(x, y, 1, 1);
        }
      }
    }
  }
  ctx.globalAlpha = 1;

  drawStars(ctx, t, 0, 0, w, h, 0.4);
  drawPlanet(ctx, w - Math.round(Math.min(w, h) * 0.16), h - Math.round(Math.min(w, h) * 0.12), Math.round(Math.min(w, h) * 0.13));
}

function drawPlanet(ctx: CanvasRenderingContext2D, cx: number, cy: number, r: number) {
  // Back half of the ring.
  ctx.fillStyle = "#b98c5a";
  for (let x = -r * 1.9; x <= r * 1.9; x++) {
    const y = Math.round((x * 0.18) - Math.sqrt(Math.max(0, 1 - (x / (r * 1.9)) ** 2)) * r * 0.22);
    ctx.fillRect(Math.round(cx + x), cy + y, 1, 1);
  }
  for (let y = -r; y <= r; y++) {
    for (let x = -r; x <= r; x++) {
      if (x * x + y * y > r * r) continue;
      // Lit from the upper left; banded.
      const lit = (-(x + y) / (r * 1.6) + 0.5);
      const band = Math.floor((y / r + 1) * 3) % 2;
      ctx.fillStyle = lit > 0.55 ? "#e3a06a" : lit > 0.25 ? (band ? "#c4774a" : "#b86b40") : "#6e3b2a";
      ctx.fillRect(cx + x, cy + y, 1, 1);
    }
  }
  // Front half of the ring.
  ctx.fillStyle = "#e8c48e";
  for (let x = -r * 1.9; x <= r * 1.9; x++) {
    const y = Math.round((x * 0.18) + Math.sqrt(Math.max(0, 1 - (x / (r * 1.9)) ** 2)) * r * 0.22);
    ctx.fillRect(Math.round(cx + x), cy + y, 1, 1);
  }
}

// ── Bridge ──────────────────────────────────────────────────────────────────

const WALL = "#232a3d";
const WALL_DARK = "#181d2c";
const FRAME = "#3c4660";
const FRAME_LIGHT = "#5d6a8a";
const DECK = "#2b3247";
const DECK_LINE = "#1f2536";
const CONSOLE = "#39425b";
const CONSOLE_TOP = "#4d5878";

/**
 * The bridge interior: a forward viewport onto streaming stars across the top
 * (faster, streaked, while the crew works), wall plating, and the deck.
 */
export function drawBridge(
  ctx: CanvasRenderingContext2D,
  t: number,
  w: number,
  h: number,
  viewportH: number,
  warp: boolean,
) {
  ctx.fillStyle = WALL;
  ctx.fillRect(0, 0, w, h);
  // Deck plating below the viewport, with seams.
  ctx.fillStyle = DECK;
  ctx.fillRect(0, viewportH + 6, w, h - viewportH - 6);
  ctx.fillStyle = DECK_LINE;
  for (let y = viewportH + 14; y < h; y += 12) ctx.fillRect(0, y, w, 1);
  for (let x = 7; x < w; x += 24) ctx.fillRect(x, viewportH + 6, 1, h);

  // Viewport: space beyond, a mullioned frame in front.
  const vx = 6;
  const vy = 4;
  const vw = w - 12;
  const vh = viewportH - 6;
  ctx.fillStyle = SKY_TOP;
  ctx.fillRect(vx, vy, vw, vh);
  drawStars(ctx, t, vx, vy, vw, vh, warp ? 9 : 1.2, warp);
  ctx.fillStyle = FRAME;
  ctx.fillRect(vx - 2, vy - 2, vw + 4, 2);
  ctx.fillRect(vx - 2, vy + vh, vw + 4, 3);
  ctx.fillRect(vx - 2, vy, 2, vh);
  ctx.fillRect(vx + vw, vy, 2, vh);
  const panes = Math.max(2, Math.round(vw / 90));
  for (let i = 1; i < panes; i++) ctx.fillRect(vx + Math.round((vw * i) / panes) - 1, vy, 2, vh);
  ctx.fillStyle = FRAME_LIGHT;
  ctx.fillRect(vx - 2, vy - 2, vw + 4, 1);
  // Glare on the glass.
  ctx.save();
  ctx.globalAlpha = 0.07;
  ctx.fillStyle = "#ffffff";
  for (let i = 0; i < vh; i++) ctx.fillRect(vx + 10 + i, vy + i, 4, 1);
  ctx.restore();
  // Wall band under the viewport, with blinking indicator lights.
  ctx.fillStyle = WALL_DARK;
  ctx.fillRect(0, viewportH + 1, w, 5);
  for (let x = 10, i = 0; x < w - 4; x += 15, i++) {
    const on = Math.sin(t / (500 + (i % 5) * 170) + i) > 0;
    ctx.fillStyle = on ? ["#4ac26b", "#e0c24a", "#4a8fe0"][i % 3] : "#141826";
    ctx.fillRect(x, viewportH + 3, 2, 1);
  }
}

/** A console bank behind one row of seats: screens at head height glowing in
 *  each seated crew member's state colour. */
export function drawConsoleBank(
  ctx: CanvasRenderingContext2D,
  t: number,
  screens: { x: number; w: number; color: string }[],
  top: number,
) {
  for (const s of screens) {
    ctx.fillStyle = CONSOLE;
    ctx.fillRect(s.x, top + 2, s.w, 13);
    ctx.fillStyle = CONSOLE_TOP;
    ctx.fillRect(s.x, top + 2, s.w, 1);
    // The screen, with a scrolling scanline.
    ctx.fillStyle = "#0b1020";
    ctx.fillRect(s.x + 3, top + 4, s.w - 6, 8);
    ctx.fillStyle = s.color;
    ctx.globalAlpha = 0.55;
    ctx.fillRect(s.x + 3, top + 4, s.w - 6, 8);
    ctx.globalAlpha = 0.9;
    const scan = top + 4 + (Math.floor(t / 180 + s.x) % 8);
    ctx.fillRect(s.x + 3, scan, s.w - 6, 1);
    ctx.globalAlpha = 1;
  }
}
