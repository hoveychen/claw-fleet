/**
 * The sea-floor backdrop: water gradient, light shafts, sand. Fixed art
 * colours on purpose — this is a painted scene, not themed chrome.
 */

export const WATER_TOP = "#1d5f86";
export const WATER_BOTTOM = "#0b2240";
export const SAND = "#c9a46a";
export const SAND_DARK = "#a9844f";

/** Height in art pixels of the sand strip along the bottom. */
export const SAND_H = 14;

export function drawOcean(ctx: CanvasRenderingContext2D, t: number, w: number, h: number) {
  const g = ctx.createLinearGradient(0, 0, 0, h);
  g.addColorStop(0, WATER_TOP);
  g.addColorStop(1, WATER_BOTTOM);
  ctx.fillStyle = g;
  ctx.fillRect(0, 0, w, h);

  // Slow light shafts from the surface, swaying a few pixels.
  ctx.save();
  ctx.globalAlpha = 0.07;
  ctx.fillStyle = "#ffffff";
  for (let i = 0; i < 5; i++) {
    const x0 = ((i + 0.5) * w) / 5 + Math.sin(t / 3000 + i) * 6;
    ctx.beginPath();
    ctx.moveTo(x0 - 6, 0);
    ctx.lineTo(x0 + 6, 0);
    ctx.lineTo(x0 + 26, h - SAND_H);
    ctx.lineTo(x0 + 2, h - SAND_H);
    ctx.closePath();
    ctx.fill();
  }
  ctx.restore();

  // Sand with a dithered top edge.
  const sy = h - SAND_H;
  ctx.fillStyle = SAND;
  ctx.fillRect(0, sy, w, SAND_H);
  ctx.fillStyle = SAND_DARK;
  for (let x = 0; x < w; x += 2) ctx.fillRect(x, sy + ((x / 2) % 2), 1, 1);
  for (let x = 3; x < w; x += 7) ctx.fillRect(x, sy + 5 + (x % 5), 1, 1);
}

// ── Ambient life ────────────────────────────────────────────────────────────

/** Cheap deterministic hash → [0, 1); keeps the reef stable across frames. */
export function hash01(n: number): number {
  const x = Math.sin(n * 127.1 + 311.7) * 43758.5453;
  return x - Math.floor(x);
}

const WEED = ["#2f8a4e", "#3fa45f", "#26703f"];

/** Kelp fronds rooted in the sand, swaying with the current. */
export function drawSeaweed(ctx: CanvasRenderingContext2D, t: number, w: number, h: number) {
  const floor = h - SAND_H;
  for (let i = 0, x = 5; x < w; i++, x += 13 + Math.floor(hash01(i) * 22)) {
    const tall = 10 + Math.floor(hash01(i + 50) * 22);
    ctx.fillStyle = WEED[i % WEED.length];
    for (let y = 0; y < tall; y++) {
      // Sway grows towards the tip; each frond has its own phase.
      const k = y / tall;
      const dx = Math.round(Math.sin(t / 1400 + i * 1.7 + y * 0.25) * 2.5 * k);
      ctx.fillRect(x + dx, floor + 2 - y, y > tall - 3 ? 1 : 2, 1);
      if (y % 5 === 3) ctx.fillRect(x + dx + (i % 2 ? 2 : -1), floor + 2 - y, 1, 1);
    }
  }
}

/** Rising bubbles from each source point; a few per source, staggered. */
export function drawBubbles(ctx: CanvasRenderingContext2D, t: number, sources: { x: number; y: number }[]) {
  ctx.save();
  ctx.fillStyle = "#d8f1ff";
  sources.forEach((src, i) => {
    for (let k = 0; k < 2; k++) {
      const seed = i * 7 + k * 3;
      const rise = 40 + hash01(seed) * 30;
      const p = ((t / 1000) * (6 + hash01(seed + 1) * 5) + hash01(seed + 2) * rise) % rise;
      const x = Math.round(src.x + Math.sin(t / 600 + seed) * 1.5 + (hash01(seed + 3) - 0.5) * 8);
      const y = Math.round(src.y - p);
      ctx.globalAlpha = 0.75 * (1 - p / rise);
      const big = hash01(seed + 4) > 0.6;
      if (big) {
        ctx.fillRect(x, y - 1, 2, 1);
        ctx.fillRect(x - 1, y, 1, 2);
        ctx.fillRect(x + 2, y, 1, 2);
        ctx.fillRect(x, y + 2, 2, 1);
      } else {
        ctx.fillRect(x, y, 1, 1);
      }
    }
  });
  ctx.restore();
}

/** Drifting caustic flecks on the sand. */
export function drawCaustics(ctx: CanvasRenderingContext2D, t: number, w: number, h: number) {
  ctx.save();
  ctx.fillStyle = "#fff3cf";
  const floor = h - SAND_H;
  for (let i = 0; i < Math.ceil(w / 18); i++) {
    const x = Math.round((hash01(i + 90) * w + t / 180) % w);
    const y = floor + 3 + Math.floor(hash01(i + 91) * (SAND_H - 5));
    ctx.globalAlpha = 0.25 + 0.25 * Math.sin(t / 700 + i);
    ctx.fillRect(x, y, 2 + (i % 2), 1);
  }
  ctx.restore();
}

// ── Pods ────────────────────────────────────────────────────────────────────

const HULL = "#3a5d78";
const HULL_DARK = "#26425a";
const HULL_EDGE = "#0c1d2b";
const RIVET = "#86a7bd";
const BRASS = "#c99a3c";

/** A submarine cabin: hull frame, rivet strip carrying the label, a glass
 *  window the crew sits behind, and a periscope. */
export function drawPod(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, labelH: number) {
  // Periscope above the top-right corner.
  ctx.fillStyle = BRASS;
  ctx.fillRect(x + w - 9, y - 5, 2, 5);
  ctx.fillRect(x + w - 9, y - 6, 5, 2);
  // Outline, then the hull with the corners cut for a rounded look.
  ctx.fillStyle = HULL_EDGE;
  ctx.fillRect(x + 1, y - 1, w - 2, h + 2);
  ctx.fillRect(x - 1, y + 1, w + 2, h - 2);
  ctx.fillRect(x, y, w, h);
  ctx.fillStyle = HULL;
  ctx.fillRect(x + 1, y, w - 2, h);
  ctx.fillRect(x, y + 1, w, h - 2);
  // Label strip with rivets.
  ctx.fillStyle = HULL_DARK;
  ctx.fillRect(x + 1, y + 1, w - 2, labelH - 2);
  ctx.fillStyle = RIVET;
  for (let rx = x + 3; rx < x + w - 3; rx += 6) {
    ctx.fillRect(rx, y + 1, 1, 1);
    ctx.fillRect(rx, y + h - 2, 1, 1);
  }
  // Window: lighter water behind glass, plus a diagonal glare.
  const wx = x + 2;
  const wy = y + labelH;
  const ww = w - 4;
  const wh = h - labelH - 3;
  ctx.fillStyle = "#1f6f96";
  ctx.fillRect(wx, wy, ww, wh);
  ctx.save();
  ctx.globalAlpha = 0.12;
  ctx.fillStyle = "#ffffff";
  for (let i = 0; i < wh && 15 + i < ww; i++) {
    ctx.fillRect(wx + 4 + i, wy + i, 3, 1);
    ctx.fillRect(wx + 12 + i, wy + i, 1, 1);
  }
  ctx.restore();
  // Brass trim along the window's sill.
  ctx.fillStyle = BRASS;
  ctx.fillRect(wx, wy + wh, ww, 1);
}
